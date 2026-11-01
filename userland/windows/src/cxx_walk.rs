//! The decisions of C++ exception handling (D177, D229) that need no running program: whether
//! a record is a C++ exception, which frame, state and data a frame handler acts for when a
//! catch block is running, which state an unwind leaves a frame in (`STATUS_LONGJMP`'s too),
//! the unwind actions between two states, the search of the try blocks around a state for the
//! first handler that catches the object, how a pointer to a part of an object is found
//! through a virtual base, and where a catch block's function resumes. Pure logic over the
//! image's C++ data (`cxx_data.rs`) and a reader of the program's objects, with host tests;
//! `win32/exception/cxx.rs` calls the program's funclets and destructors on these answers.

use crate::cxx_data::{
    self, Action, CXX_EXCEPTION, CXX_MAGIC, CatchableType, FunctionData, Handler, ThrowInfo,
    TryBlock,
};
use crate::unwind::{Image, NO_STATE, UnwindTarget};

const MAGIC_LAST: u64 = 0x1993_0522;
const MAX_UNWIND_STEPS: usize = 1 << 16;
/// A function's state before its first object is constructed or try block entered.
pub const OUTSIDE_EVERY_STATE: i32 = -1;

/// Whether an exception with `code` and `parameters` is a C++ `throw`.
pub fn is_cxx_exception(code: u32, parameters: &[u64]) -> bool {
    code == CXX_EXCEPTION
        && parameters.len() >= 3
        && (CXX_MAGIC..=MAGIC_LAST).contains(&parameters[0])
}

/// Which C++ data a function uses: `__CxxFrameHandler4`'s compressed data or
/// `__CxxFrameHandler3`'s, where, and where the function starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameData {
    pub compressed: bool,
    pub rva: u64,
    pub function_begin: u32,
}

impl FrameData {
    /// The function data this names in `image`, if it is well formed.
    pub fn read<'a>(self, image: Image<'a>) -> Option<FunctionData<'a>> {
        match self.compressed {
            true => FunctionData::compressed(image, self.rva, self.function_begin),
            false => FunctionData::fixed(image, self.rva),
        }
    }
}

/// What a frame handler needs of a catch block running on its thread: the frame of the
/// function it belongs to, the state around its try block and the function's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunningCatch {
    pub parent_frame: u64,
    pub enclosing_state: i32,
    pub data: FrameData,
}

/// Whether the handler of a running catch block's funclet stands in for the catch's function:
/// a `__CxxFrameHandler3` funclet's handler data is its function's, so it searches and unwinds
/// the function's states itself; a `__CxxFrameHandler4` funclet's data, if it has a handler at
/// all, covers only its own locals.
pub fn funclet_stands_in(catch_data: FrameData, funclet_has_handler: bool) -> bool {
    funclet_has_handler && !catch_data.compressed
}

/// What a frame handler acts for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRole {
    /// The function whose frame (establisher) is `frame`, in `state`, with `data`;
    /// `in_catch_function` when that function is a running catch block's, whose frame pointer
    /// its funclets need, rather than the frame's own.
    Function {
        frame: u64,
        state: i32,
        data: FrameData,
        in_catch_function: bool,
    },
    /// A `__CxxFrameHandler4` catch funclet's own locals: unwinding undoes its own states from
    /// `from` in the catch's function's frame `frame`; it searches nothing.
    FuncletLocals { frame: u64, from: i32 },
    /// A catch funclet that is not running here: nothing to search or unwind.
    Nothing,
}

/// What the handler of the frame whose establisher is `establisher` acts for: the frame's own
/// data `own` gives `own_state`; `funclet_catch` is the running catch block whose funclet the
/// frame is, if any, and `catch_of_frame` the running catch block whose function's frame it is.
pub fn frame_role(
    own: FrameData,
    own_state: i32,
    own_is_catch_funclet: bool,
    establisher: u64,
    funclet_catch: Option<RunningCatch>,
    catch_of_frame: Option<RunningCatch>,
) -> FrameRole {
    match funclet_catch {
        Some(catch) if funclet_stands_in(catch.data, true) => FrameRole::Function {
            frame: catch.parent_frame,
            state: match own_state {
                OUTSIDE_EVERY_STATE => catch.enclosing_state,
                state => state,
            },
            data: catch.data,
            in_catch_function: true,
        },
        Some(catch) => FrameRole::FuncletLocals {
            frame: catch.parent_frame,
            from: own_state,
        },
        None if own_is_catch_funclet => FrameRole::Nothing,
        None => FrameRole::Function {
            frame: establisher,
            state: catch_of_frame.map_or(own_state, |catch| catch.enclosing_state),
            data: own,
            in_catch_function: false,
        },
    }
}

/// The state an unwind leaves the function whose frame is `frame` in: the target's state when
/// the frame is the target's and its catch named one, the state at the target address less one
/// for a `longjmp` to that frame (its `setjmp` call's state), otherwise outside every state.
pub fn state_to_unwind_to(
    function: &FunctionData<'_>,
    function_begin: u32,
    image_base: u64,
    frame: u64,
    target: Option<&UnwindTarget>,
    long_jump: bool,
) -> i32 {
    match target {
        Some(target) if target.frame == frame && target.state != NO_STATE => target.state,
        Some(target) if target.frame == frame && long_jump => function.state_at(
            target.ip.wrapping_sub(image_base).wrapping_sub(1),
            function_begin,
        ),
        _ => OUTSIDE_EVERY_STATE,
    }
}

/// The unwind actions that take `function` from state `from` down to state `to`, innermost
/// first; a state outside the function's unwind map ends them.
pub fn unwind_actions<'a>(
    function: FunctionData<'a>,
    from: i32,
    to: i32,
) -> impl Iterator<Item = Action> + 'a {
    let mut state = from;
    let mut steps = 0;
    core::iter::from_fn(move || {
        if steps >= MAX_UNWIND_STEPS || state == to || state < 0 {
            return None;
        }
        steps += 1;
        let entry = function.unwind_entry(state)?;
        state = entry.to_state;
        Some(entry.action)
    })
}

/// What was thrown: a C++ object with its `ThrowInfo`, or a structured exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thrown {
    Cxx(ThrowInfo),
    Structured,
}

/// The first handler a search found: its try block, the handler, and the type it catches the
/// object as (none for a structured exception caught by `catch (...)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caught {
    pub block: TryBlock,
    pub handler: Handler,
    pub catchable: Option<CatchableType>,
}

/// The first handler, in the order MSVC lays them out (inner try blocks first, each block's
/// handlers in source order, each object's catchable types most derived first), of the try
/// blocks around `state` that catches `thrown`. A structured exception is caught only by
/// `catch (...)` of a function compiled for asynchronous exceptions (`/EHa`).
pub fn find_handler(
    image: &Image<'_>,
    function: &FunctionData<'_>,
    function_begin: u32,
    state: i32,
    thrown: Thrown,
) -> Option<Caught> {
    if thrown == Thrown::Structured && function.synchronous_only() {
        return None;
    }
    for index in 0..function.try_count() {
        let block = function.try_block(index)?;
        if state < block.low || state > block.high {
            continue;
        }
        for handler_index in 0..block.handler_count {
            let Some(handler) = function.handler(&block, handler_index, function_begin) else {
                break;
            };
            match thrown {
                Thrown::Structured if handler.type_descriptor == 0 => {
                    return Some(Caught {
                        block,
                        handler,
                        catchable: None,
                    });
                }
                Thrown::Structured => {}
                Thrown::Cxx(info) => {
                    for type_index in 0..info.catchable_count {
                        let Some(catchable) = cxx_data::catchable_type(image, &info, type_index)
                        else {
                            break;
                        };
                        if cxx_data::handler_catches(image, &handler, &catchable, info.attributes)
                        {
                            return Some(Caught {
                                block,
                                handler,
                                catchable: Some(catchable),
                            });
                        }
                    }
                }
            }
        }
    }
    None
}

/// Reads the program's objects: a virtual base table's address in an object, an offset in the
/// table, a virtual function table's slots.
pub trait ObjectMemory {
    /// The 64-bit word at `address`.
    fn u64_at(&self, address: u64) -> u64;
    /// The 32-bit signed word at `address`.
    fn i32_at(&self, address: u64) -> i32;
    /// The byte at `address`.
    fn u8_at(&self, address: u64) -> u8;
}

/// How to find a part of an object from the object's address (MSVC's `PMD`): add `member`;
/// with a virtual base (`virtual_base_pointer` not negative), first move by the offset the
/// object's virtual base table holds at `virtual_base`, the table's address being at
/// `virtual_base_pointer` in the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Displacement {
    pub member: i32,
    pub virtual_base_pointer: i32,
    pub virtual_base: i32,
}

impl Displacement {
    /// The displacement of the part a thrown object is caught as.
    pub fn of(catchable: &CatchableType) -> Self {
        Self {
            member: catchable.member_displacement,
            virtual_base_pointer: catchable.virtual_base_pointer,
            virtual_base: catchable.virtual_base_displacement,
        }
    }

    /// The address of the part of the object at `object`.
    pub fn apply(self, object: u64, memory: &impl ObjectMemory) -> u64 {
        let mut result = object.wrapping_add(self.member as i64 as u64);
        if self.virtual_base_pointer >= 0 {
            let base_pointer = object.wrapping_add(self.virtual_base_pointer as u64);
            let table = memory.u64_at(base_pointer);
            let offset = memory.i32_at(table.wrapping_add(self.virtual_base as i64 as u64));
            result = result
                .wrapping_add(offset as i64 as u64)
                .wrapping_add(self.virtual_base_pointer as u64);
        }
        result
    }
}

/// Where a catch block's function resumes after the funclet returned `returned`: an index
/// into the handler's continuation addresses when it is below their count, else the address.
pub fn continuation(handler: &Handler, returned: u64) -> u64 {
    match usize::try_from(returned) {
        Ok(index) if index < handler.continuation_count => handler.continuations[index],
        _ => returned,
    }
}

/// Whether an exception leaving a catch block destroys the object the block caught
/// (`caught_object`): unless the leaving exception is that C++ object rethrown.
pub fn leaving_destroys_caught(
    caught_object: u64,
    leaving_is_cxx: bool,
    leaving_object: u64,
) -> bool {
    !(leaving_is_cxx && leaving_object == caught_object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cxx_data::flags;
    use std::collections::BTreeMap;

    const BASE: u64 = 0x1_4000_0000;
    const BEGIN: u32 = 0x1000;
    const EHS: u32 = 1;

    fn put(bytes: &mut [u8], at: usize, words: &[u32]) {
        for (index, word) in words.iter().enumerate() {
            bytes[at + index * 4..at + index * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
    }

    fn function_bytes(flags_word: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x600];
        put(
            &mut bytes,
            0x100,
            &[0x1993_0522, 4, 0x200, 2, 0x240, 5, 0x2c0, 0, 0, flags_word],
        );
        put(&mut bytes, 0x200, &[u32::MAX, 0, 0, 0x1500, 1, 0, 2, 0x1510]);
        put(&mut bytes, 0x240, &[2, 3, 3, 1, 0x280]);
        put(&mut bytes, 0x254, &[1, 3, 3, 2, 0x290]);
        put(&mut bytes, 0x280, &[flags::HANDLER_REFERENCE, 0x400, 0x28, 0x1600, 0]);
        put(&mut bytes, 0x290, &[flags::HANDLER_REFERENCE, 0x4c0, 0x30, 0x1700, 0]);
        put(&mut bytes, 0x2a4, &[0, 0, 0, 0x1800, 0]);
        put(
            &mut bytes,
            0x2c0,
            &[0x1000, u32::MAX, 0x1010, 0, 0x1020, 1, 0x1030, 2, 0x1040, 3],
        );
        put(&mut bytes, 0x300, &[0, 0x1900, 0, 0x320]);
        put(&mut bytes, 0x320, &[2, 0x340, 0x360]);
        put(&mut bytes, 0x340, &[0, 0x480, 0, u32::MAX, 0, 24, 0x1a00]);
        put(&mut bytes, 0x360, &[0, 0x440, 8, u32::MAX, 0, 16, 0x1b00]);
        bytes[0x410..0x41a].copy_from_slice(b".?AVBase@@");
        bytes[0x450..0x45a].copy_from_slice(b".?AVBase@@");
        bytes[0x490..0x49d].copy_from_slice(b".?AVDerived@@");
        bytes[0x4d0..0x4db].copy_from_slice(b".?AVOther@@");
        bytes
    }

    fn data() -> FrameData {
        FrameData {
            compressed: false,
            rva: 0x100,
            function_begin: BEGIN,
        }
    }

    fn image(bytes: &[u8]) -> Image<'_> {
        Image { base: BASE, bytes }
    }

    #[test]
    fn only_records_with_the_cxx_code_and_magic_are_cxx_exceptions() {
        assert!(is_cxx_exception(CXX_EXCEPTION, &[CXX_MAGIC, 1, 2, 3]));
        assert!(is_cxx_exception(CXX_EXCEPTION, &[0x1993_0522, 1, 2]));
        assert!(!is_cxx_exception(CXX_EXCEPTION, &[0x1993_0523, 1, 2]));
        assert!(!is_cxx_exception(CXX_EXCEPTION, &[CXX_MAGIC, 1]));
        assert!(!is_cxx_exception(0xc000_0005, &[CXX_MAGIC, 1, 2]));
    }

    #[test]
    fn the_inner_try_block_catches_by_the_base_class_s_name_before_the_outer_one() {
        let bytes = function_bytes(EHS);
        let image = image(&bytes);
        let function = data().read(image).unwrap();
        let info = cxx_data::throw_info(&image, BASE + 0x300).unwrap();
        let caught = find_handler(&image, &function, BEGIN, 3, Thrown::Cxx(info)).unwrap();
        assert_eq!((caught.block.low, caught.block.high), (2, 3));
        assert_eq!(caught.handler.funclet, BASE + 0x1600);
        let base = caught.catchable.unwrap();
        assert_eq!(base.type_descriptor, BASE + 0x440);
        assert_eq!(base.member_displacement, 8);
        assert_eq!(caught.handler.type_descriptor, BASE + 0x400);
        let outer = find_handler(&image, &function, BEGIN, 1, Thrown::Cxx(info)).unwrap();
        assert_eq!(outer.handler.funclet, BASE + 0x1800);
        assert_eq!(
            outer.catchable.map(|derived| derived.type_descriptor),
            Some(BASE + 0x480)
        );
        assert_eq!(find_handler(&image, &function, BEGIN, 0, Thrown::Cxx(info)), None);
        assert_eq!(find_handler(&image, &function, BEGIN, 4, Thrown::Cxx(info)), None);
    }

    #[test]
    fn a_structured_exception_reaches_only_catch_all_under_asynchronous_handling() {
        let synchronous = function_bytes(EHS);
        let image_s = image(&synchronous);
        let function = data().read(image_s).unwrap();
        assert_eq!(find_handler(&image_s, &function, BEGIN, 1, Thrown::Structured), None);
        let asynchronous = function_bytes(0);
        let image_a = image(&asynchronous);
        let function = data().read(image_a).unwrap();
        let caught = find_handler(&image_a, &function, BEGIN, 1, Thrown::Structured).unwrap();
        assert_eq!(caught.handler.funclet, BASE + 0x1800);
        assert_eq!(caught.catchable, None);
        assert_eq!(
            find_handler(&image_a, &function, BEGIN, 3, Thrown::Structured)
                .map(|caught| caught.handler.funclet),
            Some(BASE + 0x1800)
        );
    }

    #[test]
    fn unwinding_runs_each_state_s_action_down_to_the_target_state() {
        let bytes = function_bytes(EHS);
        let function = data().read(image(&bytes)).unwrap();
        let all: Vec<Action> = unwind_actions(function, 3, OUTSIDE_EVERY_STATE).collect();
        assert_eq!(
            all,
            [
                Action::Funclet(BASE + 0x1510),
                Action::Nothing,
                Action::Funclet(BASE + 0x1500),
                Action::Nothing
            ]
        );
        let to_try: Vec<Action> = unwind_actions(function, 3, 1).collect();
        assert_eq!(to_try, [Action::Funclet(BASE + 0x1510), Action::Nothing]);
        assert_eq!(unwind_actions(function, 1, 1).count(), 0);
        assert_eq!(unwind_actions(function, OUTSIDE_EVERY_STATE, 0).count(), 0);
        assert_eq!(unwind_actions(function, 9, OUTSIDE_EVERY_STATE).count(), 0);
    }

    #[test]
    fn an_unwind_stops_at_the_catch_s_state_or_the_setjmp_call_s_state_in_its_target_frame() {
        let bytes = function_bytes(EHS);
        let function = data().read(image(&bytes)).unwrap();
        let frame = 0x7fff_0000;
        let catch_target = UnwindTarget {
            frame,
            ip: BASE + 0x1600,
            state: 2,
        };
        let state = |target: Option<&UnwindTarget>, frame, long_jump| {
            state_to_unwind_to(&function, BEGIN, BASE, frame, target, long_jump)
        };
        assert_eq!(state(Some(&catch_target), frame, false), 2);
        assert_eq!(state(Some(&catch_target), frame + 16, false), -1);
        assert_eq!(state(None, frame, false), -1);
        let long_jump = UnwindTarget {
            frame,
            ip: BASE + 0x1035,
            state: NO_STATE,
        };
        assert_eq!(state(Some(&long_jump), frame, true), 2);
        assert_eq!(state(Some(&long_jump), frame, false), -1);
    }

    fn running(compressed: bool) -> RunningCatch {
        RunningCatch {
            parent_frame: 0x7000,
            enclosing_state: 1,
            data: FrameData {
                compressed,
                rva: 0x900,
                function_begin: 0x2000,
            },
        }
    }

    #[test]
    fn a_fh3_catch_funclet_s_handler_acts_for_its_function_from_the_try_s_enclosing_state() {
        let catch = running(false);
        assert_eq!(
            frame_role(data(), -1, true, 0x6000, Some(catch), None),
            FrameRole::Function {
                frame: 0x7000,
                state: 1,
                data: catch.data,
                in_catch_function: true
            }
        );
        assert_eq!(
            frame_role(data(), 3, true, 0x6000, Some(catch), None),
            FrameRole::Function {
                frame: 0x7000,
                state: 3,
                data: catch.data,
                in_catch_function: true
            }
        );
    }

    #[test]
    fn a_fh4_catch_funclet_only_undoes_its_own_locals_and_an_idle_funclet_nothing() {
        assert_eq!(
            frame_role(data(), 2, true, 0x6000, Some(running(true)), None),
            FrameRole::FuncletLocals {
                frame: 0x7000,
                from: 2
            }
        );
        assert_eq!(
            frame_role(data(), 2, true, 0x6000, None, None),
            FrameRole::Nothing
        );
    }

    #[test]
    fn a_function_whose_catch_is_running_is_searched_from_the_try_s_enclosing_state() {
        assert_eq!(
            frame_role(data(), 3, false, 0x7000, None, Some(running(true))),
            FrameRole::Function {
                frame: 0x7000,
                state: 1,
                data: data(),
                in_catch_function: false
            }
        );
        assert_eq!(
            frame_role(data(), 3, false, 0x7000, None, None),
            FrameRole::Function {
                frame: 0x7000,
                state: 3,
                data: data(),
                in_catch_function: false
            }
        );
        assert!(funclet_stands_in(running(false).data, true));
        assert!(!funclet_stands_in(running(false).data, false));
        assert!(!funclet_stands_in(running(true).data, true));
    }

    struct Objects(BTreeMap<u64, u64>);

    impl ObjectMemory for Objects {
        fn u64_at(&self, address: u64) -> u64 {
            self.0.get(&address).copied().unwrap_or(0)
        }

        fn i32_at(&self, address: u64) -> i32 {
            self.0.get(&address).copied().unwrap_or(0) as i32
        }

        fn u8_at(&self, address: u64) -> u8 {
            self.0.get(&address).copied().unwrap_or(0) as u8
        }
    }

    #[test]
    fn a_virtual_base_is_found_through_the_object_s_virtual_base_table() {
        let object = 0x5000;
        let memory = Objects(BTreeMap::from([(0x5008, 0x9000), (0x9004, 0x20)]));
        let plain = Displacement {
            member: 8,
            virtual_base_pointer: -1,
            virtual_base: 0,
        };
        assert_eq!(plain.apply(object, &memory), 0x5008);
        let virtual_base = Displacement {
            member: 0,
            virtual_base_pointer: 8,
            virtual_base: 4,
        };
        assert_eq!(virtual_base.apply(object, &memory), 0x5000 + 8 + 0x20);
        let negative = Displacement {
            member: -8,
            virtual_base_pointer: -1,
            virtual_base: 0,
        };
        assert_eq!(negative.apply(object, &memory), 0x4ff8);
    }

    #[test]
    fn a_funclet_names_a_continuation_by_index_or_by_address() {
        let handler = Handler {
            adjectives: 0,
            type_descriptor: 0,
            catch_object: 0,
            funclet: 0,
            continuations: [BASE + 0x1050, BASE + 0x1060],
            continuation_count: 2,
        };
        assert_eq!(continuation(&handler, 0), BASE + 0x1050);
        assert_eq!(continuation(&handler, 1), BASE + 0x1060);
        assert_eq!(continuation(&handler, BASE + 0x1070), BASE + 0x1070);
        let fixed = Handler {
            continuation_count: 0,
            ..handler
        };
        assert_eq!(continuation(&fixed, 0), 0);
    }

    #[test]
    fn leaving_a_catch_keeps_its_object_only_when_it_is_rethrown() {
        assert!(!leaving_destroys_caught(0x5000, true, 0x5000));
        assert!(leaving_destroys_caught(0x5000, true, 0x6000));
        assert!(leaving_destroys_caught(0x5000, false, 0x5000));
    }
}
