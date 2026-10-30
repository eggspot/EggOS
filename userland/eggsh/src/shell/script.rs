//! The shell's grammar (D182), walked over the text itself: a complete command is checked
//! whole first (`Pass::Check`, which also finds its here-documents' bodies), then run
//! (`Pass::Run`); parts that do not run (a branch not taken, a pipeline after a failed
//! `&&`) are read again without expanding anything (`Pass::Skip`). Loops read their body's
//! text again on each round, and functions keep their text, so nothing needs memory beyond the
//! text and fixed buffers. Subshells, `$(...)`, background lists and compound commands in a
//! pipeline run in a forked copy of the shell.

use super::{
    Errno, Forked, Group, LINE_CAPACITY, MAX_CALL_DEPTH, NULL_DEVICE, Pipeline, Ran, Read, Shell,
    Streams, System,
};
use crate::functions::FUNCTIONS_CAPACITY;
use crate::glob::{self, Directories, MAX_MATCHES, Matches, Unreadable};
use crate::path::{Kind, PATH_CAPACITY};
use crate::pattern;
use crate::status::Change;
use crate::syntax::{
    self, Keyword, NoParameters, PATTERN_CAPACITY, Parsed, Redirection, SyntaxError, Terminator,
    Words, is_name_byte, is_name_start,
};

/// The most here-documents one complete command has.
pub const MAX_DOCUMENTS: usize = 16;

const SUBJECT_CAPACITY: usize = 4096;

const PIPE_PIECE: usize = 512;

/// Text the grammar walks: all of it there, or more may come (`complete` false).
#[derive(Debug, Clone, Copy)]
pub(super) struct Source<'t> {
    text: &'t [u8],
    complete: bool,
}

impl<'t> Source<'t> {
    pub(super) fn new(text: &'t [u8], complete: bool) -> Self {
        Self { text, complete }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    Check,
    Skip,
    Run,
}

impl Pass {
    fn quiet(self) -> Self {
        match self {
            Self::Check => Self::Check,
            _ => Self::Skip,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    End,
    Newline,
    Word(Keyword),
    Close,
    CaseEnd,
}

impl Stop {
    fn token(self) -> &'static str {
        match self {
            Self::End | Self::Newline => "newline",
            Self::Word(keyword) => keyword.text(),
            Self::Close => ")",
            Self::CaseEnd => ";;",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compound {
    Brace,
    Subshell,
    If,
    While,
    Until,
    For,
    Case,
}

/// `break`, `continue` or `return` on its way out of loops and calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Control {
    Break(u32),
    Continue(u32),
    Return,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Next,
    Again,
    Leave,
}

#[derive(Debug, Clone, Copy, Default)]
struct Document {
    operator: usize,
    body: usize,
    end: usize,
    after: usize,
}

/// The here-documents of the complete command being run: where each one's body is.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Documents {
    found: [Document; MAX_DOCUMENTS],
    count: usize,
    waiting: [usize; MAX_DOCUMENTS],
    waiting_count: usize,
}

impl Documents {
    fn wait_for(&mut self, operator: usize) -> Result<(), SyntaxError> {
        if self.waiting_count + self.count == MAX_DOCUMENTS {
            return Err(SyntaxError::TooLong);
        }
        self.waiting[self.waiting_count] = operator;
        self.waiting_count += 1;
        Ok(())
    }

    fn next_waiting(&mut self) -> Option<usize> {
        if self.waiting_count == 0 {
            return None;
        }
        let first = self.waiting[0];
        self.waiting.copy_within(1..self.waiting_count, 0);
        self.waiting_count -= 1;
        Some(first)
    }

    fn add(&mut self, document: Document) {
        self.found[self.count] = document;
        self.count += 1;
    }

    fn starting_at(&self, body: usize) -> Option<Document> {
        self.found[..self.count]
            .iter()
            .find(|document| document.body == body)
            .copied()
    }

    fn of(&self, operator: usize) -> Option<Document> {
        self.found[..self.count]
            .iter()
            .find(|document| document.operator == operator)
            .copied()
    }
}

struct SystemDirectories<'s, S>(&'s mut S);

impl<S: System> Directories for SystemDirectories<'_, S> {
    fn read_directory(
        &mut self,
        path: &[u8],
        each: &mut dyn FnMut(&[u8]),
    ) -> Result<(), Unreadable> {
        self.0.read_directory(path, each).map_err(|_| Unreadable)
    }

    fn exists(&mut self, path: &[u8]) -> bool {
        self.0.kind(path) != Kind::Missing
    }
}

fn skip_blanks(text: &[u8], mut at: usize) -> usize {
    while matches!(text.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    at
}

fn is_closer(keyword: Keyword) -> bool {
    matches!(
        keyword,
        Keyword::Then
            | Keyword::Elif
            | Keyword::Else
            | Keyword::Fi
            | Keyword::Do
            | Keyword::Done
            | Keyword::Esac
            | Keyword::CloseBrace
    )
}

fn expect(source: Source<'_>, stop: Stop, wanted: Stop) -> Result<(), SyntaxError> {
    match stop == wanted {
        true => Ok(()),
        false => Err(mismatch(source, stop, wanted)),
    }
}

fn mismatch(source: Source<'_>, stop: Stop, wanted: Stop) -> SyntaxError {
    match stop {
        Stop::End if source.complete => SyntaxError::Missing(wanted.token()),
        Stop::End => SyntaxError::Incomplete,
        _ => SyntaxError::UnexpectedToken(stop.token()),
    }
}

fn wanted(source: Source<'_>, at: usize, word: &'static str) -> SyntaxError {
    match (at >= source.text.len(), source.complete) {
        (true, true) => SyntaxError::Missing(word),
        (true, false) => SyntaxError::Incomplete,
        (false, _) => SyntaxError::Expected(word),
    }
}

fn pattern_matches(words: &Words, subject: &[u8]) -> bool {
    if words.count() == 0 {
        return subject.is_empty();
    }
    let mut buffer = [0u8; PATTERN_CAPACITY];
    match words.pattern(0, &mut buffer) {
        Some(pattern) => pattern::matches(pattern, subject),
        None => false,
    }
}

fn unescape_backquoted<'b>(command: &[u8], buffer: &'b mut [u8]) -> Option<&'b [u8]> {
    let mut length = 0;
    let mut at = 0;
    while at < command.len() {
        let byte = match (command[at], command.get(at + 1)) {
            (b'\\', Some(&next @ (b'$' | b'`' | b'\\'))) => {
                at += 1;
                next
            }
            (byte, _) => byte,
        };
        *buffer.get_mut(length)? = byte;
        length += 1;
        at += 1;
    }
    Some(&buffer[..length])
}

impl<S: System> Shell<'_, S> {
    /// Checks the complete command at `at` in `source`: where it ends, its newline and
    /// here-documents included.
    pub(super) fn check(
        &mut self,
        source: Source<'_>,
        at: usize,
        words: &mut Words,
    ) -> Result<usize, SyntaxError> {
        let (end, stop) = self.list(source, at, Pass::Check, false, words)?;
        match stop {
            Stop::Newline | Stop::End => Ok(end),
            other => Err(SyntaxError::UnexpectedToken(other.token())),
        }
    }

    /// Runs the complete command at `at`, checked before.
    pub(super) fn run_complete(
        &mut self,
        source: Source<'_>,
        at: usize,
        words: &mut Words,
    ) -> Result<(), SyntaxError> {
        self.list(source, at, Pass::Run, false, words).map(|_| ())
    }

    fn read_command(
        &mut self,
        source: Source<'_>,
        at: usize,
        pass: Pass,
        plain: bool,
        words: &mut Words,
    ) -> Result<Parsed, SyntaxError> {
        let input = &source.text[at..];
        match (pass, plain) {
            (Pass::Run, false) => syntax::next_command(input, source.complete, self, words),
            (Pass::Run, true) => syntax::next_words(input, source.complete, self, words),
            (_, false) => syntax::next_command(input, source.complete, &mut NoParameters, words),
            (_, true) => syntax::next_words(input, source.complete, &mut NoParameters, words),
        }
    }

    fn read_trailer(
        &mut self,
        source: Source<'_>,
        at: usize,
        pass: Pass,
        words: &mut Words,
    ) -> Result<Parsed, SyntaxError> {
        let input = &source.text[at..];
        let parsed = match pass {
            Pass::Run => syntax::next_trailer(input, source.complete, self, words),
            _ => syntax::next_trailer(input, source.complete, &mut NoParameters, words),
        }?;
        if words.count() > 0 {
            return Err(SyntaxError::UnexpectedWord);
        }
        Ok(parsed)
    }

    fn read_word(
        &mut self,
        source: Source<'_>,
        at: usize,
        pass: Pass,
        words: &mut Words,
    ) -> Result<Option<usize>, SyntaxError> {
        let input = &source.text[at..];
        let read = match pass {
            Pass::Run => syntax::next_word(input, source.complete, self, words),
            _ => syntax::next_word(input, source.complete, &mut NoParameters, words),
        };
        Ok(read?.map(|end| at + end))
    }

    fn note_documents(&mut self, start: usize, words: &Words) -> Result<(), SyntaxError> {
        for redirect in words.redirections() {
            if let syntax::Redirect::Document { position, .. } = redirect {
                self.documents.wait_for(start + position)?;
            }
        }
        Ok(())
    }

    fn separators(
        &mut self,
        source: Source<'_>,
        mut at: usize,
        pass: Pass,
        newlines: bool,
    ) -> Result<usize, SyntaxError> {
        let text = source.text;
        loop {
            match text.get(at) {
                Some(b' ' | b'\t') => at += 1,
                Some(b'\\') if text.get(at + 1) == Some(&b'\n') => at += 2,
                Some(b'\\') if at + 1 == text.len() && !source.complete => {
                    return Err(SyntaxError::Incomplete);
                }
                Some(b'#') => match text[at..].iter().position(|&byte| byte == b'\n') {
                    Some(newline) => at += newline,
                    None if source.complete => at = text.len(),
                    None => return Err(SyntaxError::Incomplete),
                },
                Some(b'\n') if newlines => at = self.after_newline(source, at + 1, pass)?,
                _ => return Ok(at),
            }
        }
    }

    fn after_newline(
        &mut self,
        source: Source<'_>,
        mut at: usize,
        pass: Pass,
    ) -> Result<usize, SyntaxError> {
        let text = source.text;
        if pass != Pass::Check {
            while let Some(document) = self.documents.starting_at(at) {
                at = document.after;
            }
            return Ok(at);
        }
        while let Some(operator) = self.documents.next_waiting() {
            let delimiter = syntax::document_delimiter(&text[operator..])?;
            let ends = syntax::document_end(
                text,
                at,
                delimiter.text(),
                delimiter.strip_tabs,
                source.complete,
            );
            let (end, after) = match ends {
                Some(ends) => ends,
                None if source.complete => (text.len(), text.len()),
                None => return Err(SyntaxError::Incomplete),
            };
            self.documents.add(Document {
                operator,
                body: at,
                end,
                after,
            });
            at = after;
        }
        Ok(at)
    }

    fn list(
        &mut self,
        source: Source<'_>,
        mut at: usize,
        pass: Pass,
        nested: bool,
        words: &mut Words,
    ) -> Result<(usize, Stop), SyntaxError> {
        let text = source.text;
        loop {
            at = self.separators(source, at, pass, nested)?;
            if pass == Pass::Run && self.unwinding() {
                return Ok((at, Stop::End));
            }
            match text.get(at) {
                None if source.complete => return Ok((at, Stop::End)),
                None => return Err(SyntaxError::Incomplete),
                Some(b'\n') => {
                    let after = self.after_newline(source, at + 1, pass)?;
                    return Ok((after, Stop::Newline));
                }
                Some(b')') => return Ok((at, Stop::Close)),
                Some(b';') if text.get(at + 1) == Some(&b';') => return Ok((at, Stop::CaseEnd)),
                _ => {}
            }
            if let Some(keyword) = syntax::keyword(&text[at..], source.complete)?
                && is_closer(keyword)
            {
                return Ok((at, Stop::Word(keyword)));
            }
            let (next, terminator) = self.and_or(source, at, pass, words)?;
            at = next;
            match terminator {
                Terminator::Newline if !nested => {
                    let after = self.after_newline(source, at, pass)?;
                    return Ok((after, Stop::Newline));
                }
                Terminator::Newline => at = self.after_newline(source, at, pass)?,
                Terminator::End => return Ok((at, Stop::End)),
                Terminator::Close => return Ok((at, Stop::Close)),
                Terminator::CaseEnd => return Ok((at, Stop::CaseEnd)),
                _ => {}
            }
        }
    }

    fn and_or(
        &mut self,
        source: Source<'_>,
        at: usize,
        pass: Pass,
        words: &mut Words,
    ) -> Result<(usize, Terminator), SyntaxError> {
        if pass != Pass::Run {
            let (end, terminator, _) = self.and_or_walk(source, at, pass, false, words)?;
            return Ok((end, terminator));
        }
        let (end, terminator, pipelines) = self.and_or_walk(source, at, Pass::Skip, false, words)?;
        let background = terminator == Terminator::Background;
        if background && pipelines > 1 {
            self.run_list_in_background(source, at, end - terminator.length(), words);
            return Ok((end, terminator));
        }
        let (end, terminator, _) = self.and_or_walk(source, at, Pass::Run, background, words)?;
        Ok((end, terminator))
    }

    fn and_or_walk(
        &mut self,
        source: Source<'_>,
        mut at: usize,
        pass: Pass,
        background: bool,
        words: &mut Words,
    ) -> Result<(usize, Terminator, usize), SyntaxError> {
        let mut pipelines = 0;
        let mut runs = pass == Pass::Run;
        loop {
            let this = if runs { Pass::Run } else { pass.quiet() };
            let (next, terminator, checked) = self.pipeline(source, at, this, background, words)?;
            pipelines += 1;
            at = next;
            if pass == Pass::Run && self.unwinding() {
                return Ok((at, terminator, pipelines));
            }
            match terminator {
                Terminator::And | Terminator::Or => {
                    if pass == Pass::Run {
                        runs = (self.status == 0) == (terminator == Terminator::And);
                    }
                    at = self.separators(source, at, pass, true)?;
                }
                _ => {
                    let failed = checked && self.status != 0;
                    if failed && self.errexit && self.conditions == 0 && !background {
                        self.exit = Some(self.status);
                    }
                    return Ok((at, terminator, pipelines));
                }
            }
        }
    }

    fn pipeline(
        &mut self,
        source: Source<'_>,
        at: usize,
        pass: Pass,
        background: bool,
        words: &mut Words,
    ) -> Result<(usize, Terminator, bool), SyntaxError> {
        let text = source.text;
        let mut at = skip_blanks(text, at);
        let mut negated = false;
        if syntax::keyword(&text[at..], source.complete)? == Some(Keyword::Bang) {
            negated = true;
            at = skip_blanks(text, at + 1);
        }
        let run = pass == Pass::Run;
        let mut pipeline = Pipeline::new(at);
        pipeline.background = background && run;
        loop {
            let (next, terminator) = match self.command(source, at, pass, &mut pipeline, words) {
                Ok(read) => read,
                Err(error) => {
                    if run {
                        self.finish(&mut pipeline, b"");
                    }
                    return Err(error);
                }
            };
            if terminator == Terminator::Pipe && !(run && self.unwinding()) {
                at = self.separators(source, next, pass, true)?;
                continue;
            }
            if run {
                let end = next - terminator.length();
                let line = text[pipeline.text_start..end].trim_ascii_end();
                match pipeline.background {
                    true => self.run_in_background(&mut pipeline, line),
                    false => self.finish(&mut pipeline, line),
                }
                if negated && !background {
                    self.status = i32::from(self.status == 0);
                }
            }
            return Ok((next, terminator, run && !negated));
        }
    }

    fn command(
        &mut self,
        source: Source<'_>,
        at: usize,
        pass: Pass,
        pipeline: &mut Pipeline,
        words: &mut Words,
    ) -> Result<(usize, Terminator), SyntaxError> {
        let text = source.text;
        let at = skip_blanks(text, at);
        match text.get(at) {
            None if !source.complete => return Err(SyntaxError::Incomplete),
            None | Some(b'\n') => return Err(SyntaxError::UnexpectedToken("newline")),
            Some(b'(') => {
                return self.compound(source, at, Compound::Subshell, pass, pipeline, words);
            }
            Some(b')') => return Err(SyntaxError::UnexpectedToken(")")),
            Some(b';') if text.get(at + 1) == Some(&b';') => {
                return Err(SyntaxError::UnexpectedToken(";;"));
            }
            _ => {}
        }
        let compound = match syntax::keyword(&text[at..], source.complete)? {
            Some(Keyword::If) => Some(Compound::If),
            Some(Keyword::While) => Some(Compound::While),
            Some(Keyword::Until) => Some(Compound::Until),
            Some(Keyword::For) => Some(Compound::For),
            Some(Keyword::Case) => Some(Compound::Case),
            Some(Keyword::OpenBrace) => Some(Compound::Brace),
            Some(other) => return Err(SyntaxError::UnexpectedToken(other.text())),
            None => None,
        };
        if let Some(compound) = compound {
            return self.compound(source, at, compound, pass, pipeline, words);
        }
        if let Some((name, body)) = syntax::function_definition(&text[at..]) {
            return self.define_function(source, at, name, at + body, pass, pipeline, words);
        }
        self.last_substitution = None;
        let parsed = self.read_command(source, at, pass, false, words)?;
        match pass {
            Pass::Check => self.note_documents(at, words)?,
            Pass::Run => {
                self.expand_pathnames(words)?;
                let piped = parsed.terminator == Terminator::Pipe;
                self.stage(source, at, words, pipeline, piped);
            }
            Pass::Skip => {}
        }
        Ok((at + parsed.consumed, parsed.terminator))
    }

    fn expand_pathnames(&mut self, words: &mut Words) -> Result<(), SyntaxError> {
        for index in (words.assignment_count()..words.count()).rev() {
            if words.is_pattern(index) {
                self.expand_pathname(words, index)?;
            }
        }
        Ok(())
    }

    fn expand_pathname(&mut self, words: &mut Words, index: usize) -> Result<(), SyntaxError> {
        let mut pattern = [0u8; PATH_CAPACITY];
        let Some(pattern) = words.pattern(index, &mut pattern) else {
            return Ok(());
        };
        let mut found = Matches::default();
        if glob::expand(pattern, &mut SystemDirectories(&mut self.system), &mut found).is_err() {
            self.error(&[words.get(index), b": too many file names"]);
            return Err(SyntaxError::Failed);
        }
        if found.is_empty() {
            return Ok(());
        }
        let mut list: [&[u8]; MAX_MATCHES] = [b""; MAX_MATCHES];
        for (position, slot) in list[..found.len()].iter_mut().enumerate() {
            *slot = found.get(position);
        }
        if words.replace(index, &list[..found.len()]).is_err() {
            self.error(&[words.get(index), b": too many file names"]);
            return Err(SyntaxError::Failed);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn define_function(
        &mut self,
        source: Source<'_>,
        at: usize,
        name: usize,
        body: usize,
        pass: Pass,
        pipeline: &mut Pipeline,
        words: &mut Words,
    ) -> Result<(usize, Terminator), SyntaxError> {
        let text = source.text;
        let body = self.separators(source, body, pass, true)?;
        let compound = match text.get(body) {
            Some(b'(') => Compound::Subshell,
            None if !source.complete => return Err(SyntaxError::Incomplete),
            _ => match syntax::keyword(&text[body..], source.complete)? {
                Some(Keyword::If) => Compound::If,
                Some(Keyword::While) => Compound::While,
                Some(Keyword::Until) => Compound::Until,
                Some(Keyword::For) => Compound::For,
                Some(Keyword::Case) => Compound::Case,
                Some(Keyword::OpenBrace) => Compound::Brace,
                _ => return Err(SyntaxError::UnexpectedWord),
            },
        };
        let end = self.compound_body(source, body, compound, pass.quiet(), words)?;
        let parsed = self.read_trailer(source, end, pass.quiet(), words)?;
        if pass == Pass::Check {
            self.note_documents(end, words)?;
        }
        let after = end + parsed.consumed;
        if pass == Pass::Run {
            let definition = text[body..after - parsed.terminator.length()].trim_ascii_end();
            let status = match self.functions.define(&text[at..at + name], definition) {
                Ok(()) => 0,
                Err(_) => {
                    self.error(&[&text[at..at + name], b": too many functions"]);
                    1
                }
            };
            pipeline.record(Ran::Finished(status));
        }
        Ok((after, parsed.terminator))
    }

    fn compound(
        &mut self,
        source: Source<'_>,
        at: usize,
        kind: Compound,
        pass: Pass,
        pipeline: &mut Pipeline,
        words: &mut Words,
    ) -> Result<(usize, Terminator), SyntaxError> {
        let end = self.compound_body(source, at, kind, pass.quiet(), words)?;
        let parsed = self.read_trailer(source, end, pass, words)?;
        match pass {
            Pass::Check => self.note_documents(end, words)?,
            Pass::Run => {
                let piped = parsed.terminator == Terminator::Pipe;
                self.stage_compound(source, at, end, kind, words, pipeline, piped);
            }
            Pass::Skip => {}
        }
        Ok((end + parsed.consumed, parsed.terminator))
    }

    fn compound_body(
        &mut self,
        source: Source<'_>,
        at: usize,
        kind: Compound,
        pass: Pass,
        words: &mut Words,
    ) -> Result<usize, SyntaxError> {
        match kind {
            Compound::Brace | Compound::Subshell => {
                let (close, wanted) = match kind {
                    Compound::Brace => (Keyword::CloseBrace.length(), Stop::Word(Keyword::CloseBrace)),
                    _ => (1, Stop::Close),
                };
                let (end, stop) = self.list(source, at + 1, pass, true, words)?;
                if pass == Pass::Run && self.unwinding() {
                    return Ok(end);
                }
                expect(source, stop, wanted)?;
                Ok(end + close)
            }
            Compound::If => self.if_clause(source, at + Keyword::If.length(), pass, words),
            Compound::While => self.loop_clause(source, at + Keyword::While.length(), false, pass, words),
            Compound::Until => self.loop_clause(source, at + Keyword::Until.length(), true, pass, words),
            Compound::For => self.for_clause(source, at + Keyword::For.length(), pass, words),
            Compound::Case => self.case_clause(source, at + Keyword::Case.length(), pass, words),
        }
    }

    fn condition(
        &mut self,
        source: Source<'_>,
        at: usize,
        pass: Pass,
        words: &mut Words,
    ) -> Result<(usize, Stop), SyntaxError> {
        if pass != Pass::Run {
            return self.list(source, at, pass, true, words);
        }
        self.conditions += 1;
        let result = self.list(source, at, pass, true, words);
        self.conditions -= 1;
        result
    }

    fn if_clause(
        &mut self,
        source: Source<'_>,
        mut at: usize,
        pass: Pass,
        words: &mut Words,
    ) -> Result<usize, SyntaxError> {
        let run = pass == Pass::Run;
        let mut taken = false;
        loop {
            let tested = run && !taken;
            let this = if tested { Pass::Run } else { pass.quiet() };
            let (then, stop) = self.condition(source, at, this, words)?;
            if run && self.unwinding() {
                return Ok(then);
            }
            expect(source, stop, Stop::Word(Keyword::Then))?;
            let branch = tested && self.status == 0;
            let this = if branch { Pass::Run } else { pass.quiet() };
            let (next, stop) = self.list(source, then + Keyword::Then.length(), this, true, words)?;
            taken |= branch;
            if run && self.unwinding() {
                return Ok(next);
            }
            match stop {
                Stop::Word(Keyword::Elif) => at = next + Keyword::Elif.length(),
                Stop::Word(Keyword::Else) => {
                    let branch = run && !taken;
                    let this = if branch { Pass::Run } else { pass.quiet() };
                    let start = next + Keyword::Else.length();
                    let (end, stop) = self.list(source, start, this, true, words)?;
                    if run && self.unwinding() {
                        return Ok(end);
                    }
                    expect(source, stop, Stop::Word(Keyword::Fi))?;
                    return Ok(end + Keyword::Fi.length());
                }
                Stop::Word(Keyword::Fi) => {
                    if run && !taken {
                        self.status = 0;
                    }
                    return Ok(next + Keyword::Fi.length());
                }
                other => return Err(mismatch(source, other, Stop::Word(Keyword::Fi))),
            }
        }
    }

    fn loop_control(&mut self) -> Flow {
        match self.control {
            Some(Control::Break(1)) => {
                self.control = None;
                Flow::Leave
            }
            Some(Control::Break(count)) => {
                self.control = Some(Control::Break(count - 1));
                Flow::Leave
            }
            Some(Control::Continue(1)) => {
                self.control = None;
                Flow::Again
            }
            Some(Control::Continue(count)) => {
                self.control = Some(Control::Continue(count - 1));
                Flow::Leave
            }
            Some(Control::Return) => Flow::Leave,
            None if self.exit.is_some() => Flow::Leave,
            None => Flow::Next,
        }
    }

    fn loop_clause(
        &mut self,
        source: Source<'_>,
        at: usize,
        until: bool,
        pass: Pass,
        words: &mut Words,
    ) -> Result<usize, SyntaxError> {
        if pass != Pass::Run {
            let (body, stop) = self.list(source, at, pass, true, words)?;
            expect(source, stop, Stop::Word(Keyword::Do))?;
            let (done, stop) = self.list(source, body + Keyword::Do.length(), pass, true, words)?;
            expect(source, stop, Stop::Word(Keyword::Done))?;
            return Ok(done + Keyword::Done.length());
        }
        self.loops += 1;
        let mut status = 0;
        let mut end = source.text.len();
        let result = loop {
            let (body, _) = match self.condition(source, at, Pass::Run, words) {
                Ok(read) => read,
                Err(error) => break Err(error),
            };
            match self.loop_control() {
                Flow::Leave => break Ok(()),
                Flow::Again => continue,
                Flow::Next => {}
            }
            if (self.status == 0) == until {
                break Ok(());
            }
            let start = body + Keyword::Do.length();
            match self.list(source, start, Pass::Run, true, words) {
                Ok((done, _)) => end = done + Keyword::Done.length(),
                Err(error) => break Err(error),
            }
            status = self.status;
            if self.loop_control() == Flow::Leave {
                status = self.status;
                break Ok(());
            }
        };
        self.loops -= 1;
        result?;
        if self.exit.is_none() {
            self.status = status;
        }
        Ok(end)
    }

    fn for_clause(
        &mut self,
        source: Source<'_>,
        at: usize,
        pass: Pass,
        words: &mut Words,
    ) -> Result<usize, SyntaxError> {
        let text = source.text;
        let name_start = skip_blanks(text, at);
        let name_length = text[name_start..]
            .iter()
            .take_while(|&&byte| is_name_byte(byte))
            .count();
        let name_end = name_start + name_length;
        if name_end == text.len() && !source.complete {
            return Err(SyntaxError::Incomplete);
        }
        if name_length == 0 || !is_name_start(text[name_start]) {
            return Err(SyntaxError::BadName);
        }
        let mut at = self.separators(source, name_end, pass, true)?;
        let mut listed = None;
        match syntax::keyword(&text[at..], source.complete)? {
            Some(Keyword::In) => {
                let start = at + Keyword::In.length();
                let parsed = self.read_command(source, start, pass.quiet(), true, words)?;
                match parsed.terminator {
                    Terminator::Semicolon | Terminator::Newline => {}
                    Terminator::End => return Err(SyntaxError::Missing("do")),
                    other => return Err(SyntaxError::UnexpectedToken(other.token())),
                }
                listed = Some(start);
                at = start + parsed.consumed;
                if parsed.terminator == Terminator::Newline {
                    at = self.after_newline(source, at, pass)?;
                }
            }
            Some(Keyword::Do) => {}
            _ if text.get(at) == Some(&b';') => at += 1,
            _ => return Err(wanted(source, at, "do")),
        }
        let at = self.separators(source, at, pass, true)?;
        match syntax::keyword(&text[at..], source.complete)? {
            Some(Keyword::Do) => {}
            _ => return Err(wanted(source, at, "do")),
        }
        let body = at + Keyword::Do.length();
        if pass == Pass::Run {
            let name = &text[name_start..name_end];
            return self.run_for(source, name, listed, body, words);
        }
        let (done, stop) = self.list(source, body, pass, true, words)?;
        expect(source, stop, Stop::Word(Keyword::Done))?;
        Ok(done + Keyword::Done.length())
    }

    fn run_for(
        &mut self,
        source: Source<'_>,
        name: &[u8],
        listed: Option<usize>,
        body: usize,
        words: &mut Words,
    ) -> Result<usize, SyntaxError> {
        let mut values = Words::default();
        match listed {
            Some(start) => {
                self.read_command(source, start, Pass::Run, true, &mut values)?;
                self.expand_pathnames(&mut values)?;
            }
            None => {
                for number in 1..=self.arguments.count() {
                    let argument = self.arguments.get(number).unwrap_or_default();
                    values.add(argument)?;
                }
            }
        }
        self.loops += 1;
        let mut end = source.text.len();
        let mut result = Ok(());
        self.status = 0;
        for index in 0..values.count() {
            if self.variables.set(name, values.get(index), false).is_err() {
                self.error(&[b"too many variables"]);
                self.status = 1;
                break;
            }
            match self.list(source, body, Pass::Run, true, words) {
                Ok((done, _)) => end = done + Keyword::Done.length(),
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
            if self.loop_control() == Flow::Leave {
                break;
            }
        }
        self.loops -= 1;
        result.map(|()| end)
    }

    fn case_clause(
        &mut self,
        source: Source<'_>,
        at: usize,
        pass: Pass,
        words: &mut Words,
    ) -> Result<usize, SyntaxError> {
        let text = source.text;
        let run = pass == Pass::Run;
        let Some(end) = self.read_word(source, at, pass, words)? else {
            return Err(wanted(source, skip_blanks(text, at), "a word"));
        };
        let mut subject = [0u8; SUBJECT_CAPACITY];
        let mut subject_length = 0;
        if run && words.count() > 0 {
            let word = words.get(0);
            subject_length = word.len();
            subject
                .get_mut(..subject_length)
                .ok_or(SyntaxError::TooLong)?
                .copy_from_slice(word);
        }
        let mut at = self.separators(source, end, pass, true)?;
        match syntax::keyword(&text[at..], source.complete)? {
            Some(Keyword::In) => at += Keyword::In.length(),
            _ => return Err(wanted(source, at, "in")),
        }
        let mut matched = false;
        loop {
            at = self.separators(source, at, pass, true)?;
            if at == text.len() {
                return Err(match source.complete {
                    true => SyntaxError::Missing("esac"),
                    false => SyntaxError::Incomplete,
                });
            }
            if syntax::keyword(&text[at..], source.complete)? == Some(Keyword::Esac) {
                if run && !matched {
                    self.status = 0;
                }
                return Ok(at + Keyword::Esac.length());
            }
            if text[at] == b'(' {
                at += 1;
            }
            let mut chosen = false;
            loop {
                let testing = run && !matched && !chosen;
                let this = if testing { Pass::Run } else { pass.quiet() };
                let Some(end) = self.read_word(source, at, this, words)? else {
                    return Err(SyntaxError::UnexpectedToken(")"));
                };
                if testing {
                    chosen = pattern_matches(words, &subject[..subject_length]);
                }
                at = skip_blanks(text, end);
                match text.get(at) {
                    Some(b'|') => at += 1,
                    Some(b')') => {
                        at += 1;
                        break;
                    }
                    _ => return Err(wanted(source, at, ")")),
                }
            }
            let runs = run && chosen && !matched;
            if runs {
                matched = true;
                self.status = 0;
            }
            let this = if runs { Pass::Run } else { pass.quiet() };
            let (next, stop) = self.list(source, at, this, true, words)?;
            if run && self.unwinding() {
                return Ok(next);
            }
            match stop {
                Stop::CaseEnd => at = next + 2,
                Stop::Word(Keyword::Esac) => {
                    if run && !matched {
                        self.status = 0;
                    }
                    return Ok(next + Keyword::Esac.length());
                }
                other => return Err(mismatch(source, other, Stop::Word(Keyword::Esac))),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn stage_compound(
        &mut self,
        source: Source<'_>,
        at: usize,
        end: usize,
        kind: Compound,
        words: &mut Words,
        pipeline: &mut Pipeline,
        piped: bool,
    ) {
        let first = pipeline.last.is_none();
        let (mut streams, mut opened, mut ran) = self.stage_streams(pipeline, piped);
        if ran.is_none() {
            ran = self.redirect(source, end, words, &mut streams, &mut opened);
        }
        let forked = kind == Compound::Subshell || !first || piped || pipeline.background;
        let ran = ran.unwrap_or_else(|| match forked {
            true => match self.start_subshell(pipeline, streams) {
                Ok(None) => {
                    self.run_compound(source, at, kind, words);
                    self.leave_subshell();
                    Ran::Finished(self.status)
                }
                Ok(Some(child)) => Ran::Started(child),
                Err(status) => Ran::Finished(status),
            },
            false => {
                let saved = self.set_streams(streams);
                self.run_compound(source, at, kind, words);
                self.set_streams(saved);
                Ran::Finished(self.status)
            }
        });
        opened.close_all(&mut self.system);
        pipeline.record(ran);
    }

    fn run_compound(&mut self, source: Source<'_>, at: usize, kind: Compound, words: &mut Words) {
        if let Err(error) = self.compound_body(source, at, kind, Pass::Run, words) {
            self.syntax_error(error);
        }
    }

    fn start_subshell(
        &mut self,
        pipeline: &mut Pipeline,
        streams: Streams,
    ) -> Result<Option<i32>, i32> {
        let group = self.group_for(pipeline);
        match self.system.fork(group) {
            Ok(Forked::Child) => {
                self.become_subshell(streams);
                Ok(None)
            }
            Ok(Forked::Parent(child)) => {
                if self.interactive && pipeline.group.is_none() {
                    pipeline.group = Some(child);
                }
                Ok(Some(child))
            }
            Err(errno) => {
                self.error(&[b"fork: ", errno.message()]);
                Err(1)
            }
        }
    }

    fn become_subshell(&mut self, streams: Streams) {
        self.interactive = false;
        self.jobs = crate::jobs::Jobs::default();
        self.set_streams(streams);
    }

    fn leave_subshell(&mut self) {
        let status = self.exit.unwrap_or(self.status);
        self.system.end_subshell(status);
        self.control = None;
        self.exit = Some(status);
    }

    fn run_list_in_background(
        &mut self,
        source: Source<'_>,
        at: usize,
        end: usize,
        words: &mut Words,
    ) {
        let group = match self.interactive {
            true => Group::Background,
            false => Group::Shell,
        };
        let interactive = self.interactive;
        match self.system.fork(group) {
            Ok(Forked::Child) => {
                let mut streams = self.streams();
                if !interactive
                    && let Ok(null) = self
                        .system
                        .open_redirection(NULL_DEVICE, Redirection::Input)
                {
                    streams.input = null;
                }
                self.become_subshell(streams);
                let list = Source::new(&source.text[..end], true);
                if let Err(error) = self.and_or_walk(list, at, Pass::Run, false, words) {
                    self.syntax_error(error);
                }
                self.leave_subshell();
            }
            Ok(Forked::Parent(child)) => {
                let group = if interactive { child } else { 0 };
                let text = source.text[at..end].trim_ascii();
                self.add_background_job(group, &[child], Ok(child), text);
            }
            Err(errno) => {
                self.error(&[b"fork: ", errno.message()]);
                self.status = 1;
            }
        }
    }

    /// Calls the function `words` names with its other words as `$1`, `$2`, ... (D182).
    pub(super) fn call_function(&mut self, words: &mut Words, streams: Streams) -> i32 {
        if self.calls == MAX_CALL_DEPTH {
            self.error(&[words.command(0), b": functions nested too deeply"]);
            return 1;
        }
        let mut body = [0u8; FUNCTIONS_CAPACITY];
        let length = match self.functions.get(words.command(0)) {
            Some(text) => {
                body[..text.len()].copy_from_slice(text);
                text.len()
            }
            None => return 1,
        };
        let arguments = (1..words.command_length()).map(|index| words.command(index));
        if self.arguments.push_frame(arguments).is_err() {
            self.error(&[words.command(0), b": too many arguments"]);
            return 1;
        }
        let saved = self.set_streams(streams);
        let loops = core::mem::take(&mut self.loops);
        self.calls += 1;
        self.status = 0;
        self.run_buffer(&body[..length], true, words);
        self.restore_locals();
        self.calls -= 1;
        self.loops = loops;
        self.set_streams(saved);
        self.arguments.pop_frame();
        if self.control == Some(Control::Return) {
            self.control = None;
        }
        self.status
    }

    /// Runs a function call that is part of a pipeline in a forked copy of the shell.
    pub(super) fn fork_function(
        &mut self,
        words: &mut Words,
        streams: Streams,
        pipeline: &mut Pipeline,
    ) -> Ran {
        match self.start_subshell(pipeline, streams) {
            Ok(None) => {
                let status = self.call_function(words, streams);
                self.status = status;
                self.leave_subshell();
                Ran::Finished(status)
            }
            Ok(Some(child)) => Ran::Started(child),
            Err(status) => Ran::Finished(status),
        }
    }

    /// Opens the here-document whose `<<` is at `operator`: its body, expanded unless its
    /// delimiter was quoted, `<<-`'s leading tabs removed.
    pub(super) fn open_document(&mut self, source: Source<'_>, operator: usize) -> Result<usize, i32> {
        let Some(document) = self.documents.of(operator) else {
            self.error(&[b"here-document missing"]);
            return Err(1);
        };
        let Ok(delimiter) = syntax::document_delimiter(&source.text[operator..]) else {
            return Err(1);
        };
        let body = &source.text[document.body..document.end];
        let mut stripped = [0u8; LINE_CAPACITY];
        let body = match delimiter.strip_tabs {
            false => body,
            true => {
                let mut length = 0;
                for line in body.split_inclusive(|&byte| byte == b'\n') {
                    let tabs = line.iter().take_while(|&&byte| byte == b'\t').count();
                    let kept = &line[tabs..];
                    let Some(slot) = stripped.get_mut(length..length + kept.len()) else {
                        self.error(&[b"here-document too long"]);
                        return Err(1);
                    };
                    slot.copy_from_slice(kept);
                    length += kept.len();
                }
                &stripped[..length]
            }
        };
        let opened = match delimiter.quoted {
            true => self.system.document(body),
            false => {
                let mut expanded = Words::default();
                if let Err(error) = syntax::expand_document(body, self, &mut expanded) {
                    if error != SyntaxError::Failed {
                        self.error(&[b"here-document too long"]);
                    }
                    return Err(1);
                }
                self.system.document(expanded.get(0))
            }
        };
        opened.map_err(|errno: Errno| {
            self.error(&[b"here-document: ", errno.message()]);
            1
        })
    }

    /// Runs `command` in a forked copy of the shell and gives `output` what it writes:
    /// `$(...)` and `` `...` `` (D182).
    pub(super) fn run_substitution(
        &mut self,
        command: &[u8],
        backquoted: bool,
        output: &mut dyn FnMut(&[u8]) -> Result<(), SyntaxError>,
    ) -> Result<(), SyntaxError> {
        let (read, write) = match self.system.pipe() {
            Ok(ends) => ends,
            Err(errno) => {
                self.error(&[b"pipe: ", errno.message()]);
                return Err(SyntaxError::Failed);
            }
        };
        let streams = Streams {
            output: write,
            ..self.streams()
        };
        match self.system.fork(Group::Shell) {
            Ok(Forked::Child) => {
                self.system.close(read);
                self.become_subshell(streams);
                self.run_substituted(command, backquoted);
                self.leave_subshell();
                Err(SyntaxError::Failed)
            }
            Ok(Forked::Parent(child)) => {
                self.system.close(write);
                let mut piece = [0u8; PIPE_PIECE];
                let mut result = Ok(());
                loop {
                    match self.system.read_data(read, &mut piece) {
                        Read::Bytes(count) if result.is_ok() => result = output(&piece[..count]),
                        Read::Bytes(_) | Read::Interrupted => {}
                        Read::End => break,
                    }
                }
                self.system.close(read);
                if let Ok(Change::Ended(ended)) = self.wait_for(child) {
                    self.status = ended.shell_status();
                    self.last_substitution = Some(self.status);
                }
                if result == Err(SyntaxError::TooLong) {
                    self.error(&[b"command substitution: output too long"]);
                    return Err(SyntaxError::Failed);
                }
                result
            }
            Err(errno) => {
                self.system.close(read);
                self.system.close(write);
                self.error(&[b"fork: ", errno.message()]);
                Err(SyntaxError::Failed)
            }
        }
    }

    fn run_substituted(&mut self, command: &[u8], backquoted: bool) {
        let mut text = [0u8; LINE_CAPACITY];
        let command = match backquoted {
            true => match unescape_backquoted(command, &mut text) {
                Some(command) => command,
                None => {
                    self.error(&[b"command substitution too long"]);
                    self.status = 1;
                    return;
                }
            },
            false => command,
        };
        let mut words = Words::default();
        self.run_buffer(command, true, &mut words);
    }
}
