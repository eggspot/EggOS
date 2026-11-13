//! Loading definition blocks and running AML (ACPI 6.5, chapters 5 and 19): the term loop,
//! named objects, control flow and method calls. Expressions, stores and field access live in
//! the submodules.

mod convert;
mod expressions;
mod fields;
mod store;

use crate::host::Host;
use crate::name::{NameSeg, NameString, Path};
use crate::namespace::{Namespace, NodeId};
use crate::object::{
    Access, BufferField, Field, FieldKind, Method, NativeMethod, Object, Region, RegionSpace,
    Target, UpdateRule,
};
use crate::opcodes::*;
use crate::stream::Stream;
use crate::{Error, eisa_id};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

/// The deepest method calls nest.
pub const MAX_CALL_DEPTH: u32 = 64;
/// The most times one `While` runs its body before the interpreter gives up on it.
pub const MAX_LOOP_ITERATIONS: u64 = 1 << 20;
/// What the `Revision` opcode gives.
pub const INTERPRETER_REVISION: u64 = 1;
const TABLE_HEADER_BYTES: usize = 36;
const TABLE_LENGTH_AT: usize = 4;
const TABLE_REVISION_AT: usize = 8;
const FIRST_64_BIT_REVISION: u8 = 2;
const STATUS_PRESENT: u64 = 1 << 0;
const STATUS_FUNCTIONING: u64 = 1 << 3;
const STATUS_WHEN_ABSENT: u64 = 0x0f;
const ARGUMENTS_MASK: u8 = 0x07;
const SERIALIZED: u8 = 1 << 3;
const SYNC_LEVEL_SHIFT: u8 = 4;

const INTERFACES: &[&[u8]] = &[
    b"Windows 2000",
    b"Windows 2001",
    b"Windows 2001 SP1",
    b"Windows 2001.1",
    b"Windows 2001 SP2",
    b"Windows 2001.1 SP1",
    b"Windows 2006",
    b"Windows 2006.1",
    b"Windows 2006 SP1",
    b"Windows 2006 SP2",
    b"Windows 2009",
    b"Windows 2012",
    b"Windows 2013",
    b"Windows 2015",
    b"Windows 2016",
    b"Windows 2017",
    b"Windows 2017.2",
    b"Windows 2018",
    b"Windows 2018.2",
    b"Windows 2019",
    b"Windows 2020",
    b"Windows 2021",
    b"Windows 2022",
    b"Module Device",
    b"Processor Device",
    b"3.0 Thermal Model",
    b"Extended Address Space Descriptor",
    b"Processor Aggregator Device",
];

/// A device's `_STA`, as [`Interpreter::initialize_devices`] found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceStatus {
    pub path: Path,
    pub status: u64,
}

impl DeviceStatus {
    pub fn is_present(&self) -> bool {
        self.status & STATUS_PRESENT != 0
    }
}

/// One running method's state, or a table's while it loads.
#[derive(Debug)]
pub(crate) struct Frame {
    pub(crate) scope: Path,
    pub(crate) table: usize,
    pub(crate) arguments: [Object; 7],
    pub(crate) locals: [Object; 8],
    pub(crate) created: Vec<NodeId>,
    pub(crate) in_method: bool,
}

impl Frame {
    fn new(scope: Path, table: usize, in_method: bool) -> Self {
        Self {
            scope,
            table,
            arguments: Default::default(),
            locals: Default::default(),
            created: Vec::new(),
            in_method,
        }
    }
}

/// Where a term list goes on.
#[derive(Debug)]
pub(crate) enum Flow {
    Next,
    Break,
    Continue,
    Return(Object),
}

/// The namespace the loaded tables built, and what runs their methods on `H`.
#[derive(Debug)]
pub struct Interpreter<H: Host> {
    pub(crate) host: H,
    pub(crate) namespace: Namespace,
    tables: Vec<Arc<[u8]>>,
    pub(crate) integer_mask: u64,
    depth: u32,
}

pub(crate) fn opcode(stream: &mut Stream<'_>) -> Result<u16, Error> {
    let first = stream.byte()?;
    match first {
        EXTENDED_PREFIX => Ok(0x5b00 | u16::from(stream.byte()?)),
        other => Ok(u16::from(other)),
    }
}

impl<H: Host> Interpreter<H> {
    /// An interpreter with the namespace's predefined objects and no table loaded.
    pub fn new(host: H) -> Self {
        Self {
            host,
            namespace: Namespace::new(),
            tables: Vec::new(),
            integer_mask: u64::MAX,
            depth: 0,
        }
    }

    pub fn host(&mut self) -> &mut H {
        &mut self.host
    }

    pub fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    /// Whether integers are 32 bits wide: a DSDT of revision 0 or 1 says so for every table.
    pub fn has_32_bit_integers(&self) -> bool {
        self.integer_mask == u64::from(u32::MAX)
    }

    /// Loads a definition block, the DSDT or an SSDT, header included: runs its terms in the
    /// root scope, which defines its objects. What it defined before a failing term stays.
    pub fn load_table(&mut self, table: &[u8]) -> Result<(), Error> {
        let header = table.get(..TABLE_HEADER_BYTES).ok_or(Error::NotATable)?;
        let length = u32::from_le_bytes([
            header[TABLE_LENGTH_AT],
            header[TABLE_LENGTH_AT + 1],
            header[TABLE_LENGTH_AT + 2],
            header[TABLE_LENGTH_AT + 3],
        ]) as usize;
        if length < TABLE_HEADER_BYTES || length > table.len() {
            return Err(Error::NotATable);
        }
        if &header[..4] == b"DSDT" && header[TABLE_REVISION_AT] < FIRST_64_BIT_REVISION {
            self.integer_mask = u64::from(u32::MAX);
        }
        let code: Arc<[u8]> = Arc::from(&table[..length]);
        let index = self.tables.len();
        self.tables.push(Arc::clone(&code));
        let mut frame = Frame::new(Path::root(), index, false);
        let mut stream = Stream::new(&code, TABLE_HEADER_BYTES);
        self.run_terms(&mut frame, &mut stream).map(|_| ())
    }

    /// Evaluates the object at `path`: runs a method with `arguments`, reads a field, or gives
    /// any other object as it is.
    pub fn evaluate(&mut self, path: &Path, arguments: &[Object]) -> Result<Object, Error> {
        let id = self.namespace.find(path).ok_or(Error::NameNotFound)?;
        self.evaluate_node(id, arguments.to_vec())
    }

    /// [`Self::evaluate`] for the object `name` below `scope`: `None` when there is none.
    pub fn evaluate_child(
        &mut self,
        scope: &Path,
        name: &[u8; 4],
        arguments: &[Object],
    ) -> Result<Option<Object>, Error> {
        match self.namespace.find(&scope.child(NameSeg::new(name))) {
            Some(id) => self.evaluate_node(id, arguments.to_vec()).map(Some),
            None => Ok(None),
        }
    }

    /// [`Self::evaluate`], as an integer.
    pub fn evaluate_integer(&mut self, path: &Path, arguments: &[Object]) -> Result<u64, Error> {
        let value = self.evaluate(path, arguments)?;
        self.to_integer(&value)
    }

    pub(crate) fn evaluate_node(
        &mut self,
        id: NodeId,
        arguments: Vec<Object>,
    ) -> Result<Object, Error> {
        match self.namespace.get(id).cloned().ok_or(Error::NameNotFound)? {
            Object::Method(method) => self.invoke(id, &method, arguments),
            Object::NativeMethod(native) => self.native(native, &arguments),
            Object::Field(_) | Object::BufferField(_) => {
                let mut frame = Frame::new(Path::root(), 0, false);
                self.read_target(&mut frame, &Target::Node(id))
            }
            other => Ok(other),
        }
    }

    /// Runs `\_SB._INI` when there is one, then walks the namespace from the root: each device
    /// is asked its `_STA` (present and functioning when it has none), a present one runs its
    /// `_INI`, and the walk goes below a device only when it is present or functioning (ACPI
    /// 6.5, 6.5.1). A failing `_STA` or `_INI` counts its device as absent. The statuses
    /// found, in path order.
    pub fn initialize_devices(&mut self) -> Vec<DeviceStatus> {
        let bus = Path::root().child(NameSeg::new(b"_SB_"));
        let _ = self.evaluate_child(&bus, b"_INI", &[]);
        let mut found = Vec::new();
        self.initialize_below(&Path::root(), &mut found);
        found
    }

    fn initialize_below(&mut self, scope: &Path, found: &mut Vec<DeviceStatus>) {
        for child in self.namespace.children(scope) {
            let Some(path) = self.namespace.path_of(child).cloned() else {
                continue;
            };
            if path.parent().as_ref() != Some(scope) {
                continue;
            }
            let descend = match self.namespace.get(child) {
                Some(Object::Device | Object::Processor { .. } | Object::ThermalZone) => {
                    let status = self.device_status(&path);
                    let is_bus = path == Path::root().child(NameSeg::new(b"_SB_"));
                    if status & STATUS_PRESENT != 0 && !is_bus {
                        let _ = self.evaluate_child(&path, b"_INI", &[]);
                    }
                    found.push(DeviceStatus {
                        path: path.clone(),
                        status,
                    });
                    status & (STATUS_PRESENT | STATUS_FUNCTIONING) != 0
                }
                Some(Object::Scope) => true,
                _ => false,
            };
            if descend {
                self.initialize_below(&path, found);
            }
        }
    }

    /// A device's `_STA`: present, enabled, shown and functioning when it has none, absent
    /// when it fails.
    pub fn device_status(&mut self, device: &Path) -> u64 {
        match self.evaluate_child(device, b"_STA", &[]) {
            Ok(None) => STATUS_WHEN_ABSENT,
            Ok(Some(status)) => self.to_integer(&status).unwrap_or(0),
            Err(_) => 0,
        }
    }

    /// Whether the device at `path` is a PCI host bridge: its `_HID` or one of its `_CID`s is
    /// `PNP0A03` or `PNP0A08`.
    pub fn is_pci_host_bridge(&mut self, path: &Path) -> bool {
        self.device_has_id(path, &[b"PNP0A03", b"PNP0A08"])
    }

    /// Every device whose `_HID` or one of whose `_CID`s is one of `ids`, in path order.
    pub fn devices_with_id(&mut self, ids: &[&[u8; 7]]) -> Vec<Path> {
        let devices: Vec<Path> = self
            .namespace
            .iter()
            .filter(|(path, node)| {
                matches!(self.namespace.get(*node), Some(Object::Device))
                    && self.namespace.path_of(*node) == Some(*path)
            })
            .map(|(path, _)| path.clone())
            .collect();
        devices
            .into_iter()
            .filter(|path| self.device_has_id(path, ids))
            .collect()
    }

    /// Whether the device at `path` answers to one of `ids`, seven-character IDs such as
    /// `PNP0C09`, through its `_HID` or one of its `_CID`s, given as a string or a compressed
    /// EISA ID.
    pub fn device_has_id(&mut self, path: &Path, ids: &[&[u8; 7]]) -> bool {
        let names = |object: &Object| match object {
            Object::Integer(value) => ids.iter().any(|id| u64::from(eisa_id(id)) == *value),
            Object::String(text) => ids.iter().any(|id| text.as_slice() == id.as_slice()),
            _ => false,
        };
        let hid = self.evaluate_child(path, b"_HID", &[]).ok().flatten();
        if hid.as_ref().is_some_and(names) {
            return true;
        }
        match self.evaluate_child(path, b"_CID", &[]).ok().flatten() {
            Some(Object::Package(compatible)) => compatible.iter().any(names),
            Some(compatible) => names(&compatible),
            None => false,
        }
    }

    pub(crate) fn invoke(
        &mut self,
        id: NodeId,
        method: &Method,
        arguments: Vec<Object>,
    ) -> Result<Object, Error> {
        if self.depth >= MAX_CALL_DEPTH {
            return Err(Error::TooDeep);
        }
        let scope = self
            .namespace
            .path_of(id)
            .cloned()
            .ok_or(Error::NameNotFound)?;
        let code = self
            .tables
            .get(method.table)
            .cloned()
            .ok_or(Error::NotATable)?;
        let mut frame = Frame::new(scope, method.table, true);
        for (slot, argument) in frame.arguments.iter_mut().zip(arguments) {
            *slot = argument;
        }
        let body = code.get(..method.end).ok_or(Error::UnexpectedEnd)?;
        let mut stream = Stream::new(body, method.start);
        self.depth += 1;
        let result = self.run_terms(&mut frame, &mut stream);
        self.depth -= 1;
        for created in frame.created.drain(..).rev() {
            self.namespace.remove(created);
        }
        match result? {
            Flow::Return(value) => Ok(value),
            _ => Ok(Object::Uninitialized),
        }
    }

    pub(crate) fn native(
        &mut self,
        native: NativeMethod,
        arguments: &[Object],
    ) -> Result<Object, Error> {
        match native {
            NativeMethod::OperatingSystemInterface => {
                let asked = match arguments.first() {
                    Some(Object::String(text)) => text.as_slice(),
                    _ => return Err(Error::WrongType),
                };
                let answers = INTERFACES.contains(&asked);
                Ok(Object::Integer(if answers { self.integer_mask } else { 0 }))
            }
        }
    }

    pub(crate) fn run_terms(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
    ) -> Result<Flow, Error> {
        while !stream.is_done() {
            match self.term(frame, stream)? {
                Flow::Next => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Next)
    }

    fn term(&mut self, frame: &mut Frame, stream: &mut Stream<'_>) -> Result<Flow, Error> {
        if stream.at_name() {
            let name = stream.name_string()?;
            self.name_value(frame, stream, &name)?;
            return Ok(Flow::Next);
        }
        let opcode = opcode(stream)?;
        match opcode {
            NAME => {
                let name = stream.name_string()?;
                let value = self.data_reference(frame, stream)?;
                self.create(frame, &name, value)?;
            }
            ALIAS => {
                let source = stream.name_string()?;
                let alias = stream.name_string()?;
                let target = self
                    .namespace
                    .search(&source, &frame.scope)
                    .ok_or(Error::NameNotFound)?;
                let path = alias.resolve(&frame.scope).ok_or(Error::NameNotFound)?;
                self.namespace.add_alias(path, target)?;
            }
            SCOPE => {
                let end = stream.package_end()?;
                let name = stream.name_string()?;
                let path = name.resolve(&frame.scope).ok_or(Error::NameNotFound)?;
                if self.namespace.find(&path).is_none() {
                    self.namespace.add(path.clone(), Object::Scope)?;
                }
                self.run_scope(frame, stream, end, path)?;
            }
            METHOD => {
                let end = stream.package_end()?;
                let name = stream.name_string()?;
                let flags = stream.byte()?;
                let method = Method {
                    table: frame.table,
                    start: stream.at,
                    end,
                    arguments: flags & ARGUMENTS_MASK,
                    serialized: flags & SERIALIZED != 0,
                    sync_level: flags >> SYNC_LEVEL_SHIFT,
                };
                self.create(frame, &name, Object::Method(method))?;
                stream.skip_to(end);
            }
            EXTERNAL => {
                stream.name_string()?;
                stream.bytes(2)?;
            }
            DEVICE | THERMAL_ZONE => {
                let end = stream.package_end()?;
                let name = stream.name_string()?;
                let object = match opcode {
                    DEVICE => Object::Device,
                    _ => Object::ThermalZone,
                };
                let path = self.create_scope(frame, &name, object)?;
                self.run_scope(frame, stream, end, path)?;
            }
            PROCESSOR => {
                let end = stream.package_end()?;
                let name = stream.name_string()?;
                let id = stream.byte()?;
                let block_address = stream.dword()?;
                let block_length = stream.byte()?;
                let object = Object::Processor {
                    id,
                    block_address,
                    block_length,
                };
                let path = self.create_scope(frame, &name, object)?;
                self.run_scope(frame, stream, end, path)?;
            }
            POWER_RESOURCE => {
                let end = stream.package_end()?;
                let name = stream.name_string()?;
                let system_level = stream.byte()?;
                let order = stream.word()?;
                let object = Object::PowerResource {
                    system_level,
                    order,
                };
                let path = self.create_scope(frame, &name, object)?;
                self.run_scope(frame, stream, end, path)?;
            }
            OPERATION_REGION => {
                let name = stream.name_string()?;
                let space = RegionSpace::from_byte(stream.byte()?);
                let offset = self.integer_operand(frame, stream)?;
                let length = self.integer_operand(frame, stream)?;
                let region = Region {
                    space,
                    offset,
                    length,
                    pci: None,
                };
                self.create(frame, &name, Object::Region(region))?;
            }
            DATA_REGION => {
                let name = stream.name_string()?;
                let signature = self.string_operand(frame, stream)?;
                let oem_id = self.string_operand(frame, stream)?;
                let oem_table_id = self.string_operand(frame, stream)?;
                let (offset, length) = self
                    .host
                    .table_place(&signature, &oem_id, &oem_table_id)
                    .ok_or(Error::NotATable)?;
                let region = Region {
                    space: RegionSpace::SystemMemory,
                    offset,
                    length,
                    pci: None,
                };
                self.create(frame, &name, Object::Region(region))?;
            }
            FIELD => {
                let end = stream.package_end()?;
                let region = self.find_name(frame, stream)?;
                let flags = stream.byte()?;
                let mut list = stream.until(end);
                self.field_list(frame, &mut list, FieldKind::Region(region), flags)?;
                stream.skip_to(end);
            }
            INDEX_FIELD => {
                let end = stream.package_end()?;
                let index = self.find_name(frame, stream)?;
                let data = self.find_name(frame, stream)?;
                let flags = stream.byte()?;
                let mut list = stream.until(end);
                self.field_list(frame, &mut list, FieldKind::Indexed { index, data }, flags)?;
                stream.skip_to(end);
            }
            BANK_FIELD => {
                let end = stream.package_end()?;
                let region = self.find_name(frame, stream)?;
                let bank = self.find_name(frame, stream)?;
                let mut list = stream.until(end);
                let value = self.integer_operand(frame, &mut list)?;
                let flags = list.byte()?;
                let kind = FieldKind::Banked {
                    region,
                    bank,
                    value,
                };
                self.field_list(frame, &mut list, kind, flags)?;
                stream.skip_to(end);
            }
            MUTEX => {
                let name = stream.name_string()?;
                let sync_level = stream.byte()? & 0x0f;
                self.create(
                    frame,
                    &name,
                    Object::Mutex {
                        sync_level,
                        held: 0,
                    },
                )?;
            }
            EVENT => {
                let name = stream.name_string()?;
                self.create(frame, &name, Object::Event { signals: 0 })?;
            }
            CREATE_BIT_FIELD | CREATE_BYTE_FIELD | CREATE_WORD_FIELD | CREATE_DWORD_FIELD
            | CREATE_QWORD_FIELD | CREATE_FIELD => {
                self.create_buffer_field(frame, stream, opcode)?
            }
            IF => return self.if_else(frame, stream),
            ELSE => {
                let end = stream.package_end()?;
                stream.skip_to(end);
            }
            WHILE => return self.while_loop(frame, stream),
            RETURN => {
                let value = self.operand(frame, stream)?;
                return Ok(Flow::Return(value));
            }
            BREAK => return Ok(Flow::Break),
            CONTINUE => return Ok(Flow::Continue),
            NOOP | BREAK_POINT => {}
            NOTIFY => {
                let target = self.super_name(frame, stream)?;
                let value = self.integer_operand(frame, stream)?;
                let Target::Node(node) = target else {
                    return Err(Error::WrongType);
                };
                let path = self
                    .namespace
                    .path_of(node)
                    .cloned()
                    .ok_or(Error::NameNotFound)?;
                self.host.notify(&path, value);
            }
            SLEEP => {
                let milliseconds = self.integer_operand(frame, stream)?;
                self.host.sleep(milliseconds);
            }
            STALL => {
                let microseconds = self.integer_operand(frame, stream)?;
                self.host.stall(microseconds);
            }
            SIGNAL | RESET | RELEASE => {
                let target = self.super_name(frame, stream)?;
                self.synchronize(&target, opcode)?;
            }
            FATAL => {
                let kind = stream.byte()?;
                let code = stream.dword()?;
                let argument = self.integer_operand(frame, stream)?;
                return Err(Error::Fatal {
                    kind,
                    code,
                    argument,
                });
            }
            LOAD | UNLOAD => return Err(Error::Unsupported("loading tables from AML")),
            other => {
                self.operand_with(frame, stream, other)?;
            }
        }
        Ok(Flow::Next)
    }

    fn if_else(&mut self, frame: &mut Frame, stream: &mut Stream<'_>) -> Result<Flow, Error> {
        let end = stream.package_end()?;
        let mut body = stream.until(end);
        let predicate = self.integer_operand(frame, &mut body)?;
        let mut flow = match predicate {
            0 => Flow::Next,
            _ => self.run_terms(frame, &mut body)?,
        };
        stream.skip_to(end);
        if stream.peek() == Some(ELSE as u8) {
            stream.byte()?;
            let else_end = stream.package_end()?;
            if predicate == 0 {
                let mut otherwise = stream.until(else_end);
                flow = self.run_terms(frame, &mut otherwise)?;
            }
            stream.skip_to(else_end);
        }
        Ok(flow)
    }

    fn while_loop(&mut self, frame: &mut Frame, stream: &mut Stream<'_>) -> Result<Flow, Error> {
        let end = stream.package_end()?;
        let start = stream.at;
        let mut iterations = 0;
        loop {
            iterations += 1;
            if iterations > MAX_LOOP_ITERATIONS {
                return Err(Error::LoopLimit);
            }
            let mut body = stream.until(end);
            body.at = start;
            if self.integer_operand(frame, &mut body)? == 0 {
                break;
            }
            match self.run_terms(frame, &mut body)? {
                Flow::Next | Flow::Continue => {}
                Flow::Break => break,
                Flow::Return(value) => {
                    stream.skip_to(end);
                    return Ok(Flow::Return(value));
                }
            }
        }
        stream.skip_to(end);
        Ok(Flow::Next)
    }

    fn run_scope(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
        end: usize,
        path: Path,
    ) -> Result<(), Error> {
        let outer = core::mem::replace(&mut frame.scope, path);
        let mut body = stream.until(end);
        let result = self.run_terms(frame, &mut body);
        frame.scope = outer;
        stream.skip_to(end);
        result.map(|_| ())
    }

    pub(crate) fn create(
        &mut self,
        frame: &mut Frame,
        name: &NameString,
        object: Object,
    ) -> Result<NodeId, Error> {
        let path = name.resolve(&frame.scope).ok_or(Error::NameNotFound)?;
        let id = self.namespace.add(path, object)?;
        if frame.in_method {
            frame.created.push(id);
        }
        Ok(id)
    }

    fn create_scope(
        &mut self,
        frame: &mut Frame,
        name: &NameString,
        object: Object,
    ) -> Result<Path, Error> {
        let id = self.create(frame, name, object)?;
        self.namespace
            .path_of(id)
            .cloned()
            .ok_or(Error::NameNotFound)
    }

    fn find_name(&mut self, frame: &Frame, stream: &mut Stream<'_>) -> Result<NodeId, Error> {
        let name = stream.name_string()?;
        self.namespace
            .search(&name, &frame.scope)
            .ok_or(Error::NameNotFound)
    }

    fn field_list(
        &mut self,
        frame: &mut Frame,
        list: &mut Stream<'_>,
        kind: FieldKind,
        flags: u8,
    ) -> Result<(), Error> {
        const RESERVED: u8 = 0x00;
        const ACCESS: u8 = 0x01;
        const CONNECTION: u8 = 0x02;
        const EXTENDED_ACCESS: u8 = 0x03;
        let mut access = Access::from_flags(flags);
        let update = UpdateRule::from_flags(flags);
        let mut bit_offset = 0u64;
        while let Some(lead) = list.peek() {
            match lead {
                RESERVED => {
                    list.byte()?;
                    bit_offset += u64::from(list.package_length()?);
                }
                ACCESS => {
                    list.byte()?;
                    access = Access::from_flags(list.byte()?);
                    list.byte()?;
                }
                CONNECTION => {
                    list.byte()?;
                    match list.at_name() {
                        true => {
                            list.name_string()?;
                        }
                        false => {
                            self.operand(frame, list)?;
                        }
                    }
                }
                EXTENDED_ACCESS => {
                    list.byte()?;
                    access = Access::from_flags(list.byte()?);
                    list.bytes(2)?;
                }
                _ => {
                    let segment = list.bytes(4)?;
                    let segment = NameSeg([segment[0], segment[1], segment[2], segment[3]]);
                    let bit_length = u64::from(list.package_length()?);
                    let field = Field {
                        kind,
                        bit_offset,
                        bit_length,
                        access,
                        update,
                    };
                    let name = NameString {
                        root: false,
                        parents: 0,
                        segments: vec![segment],
                    };
                    self.create(frame, &name, Object::Field(field))?;
                    bit_offset += bit_length;
                }
            }
        }
        Ok(())
    }

    fn create_buffer_field(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
        opcode: u16,
    ) -> Result<(), Error> {
        let source = self.container(frame, stream)?;
        let index = self.integer_operand(frame, stream)?;
        let (bit_offset, bit_length) = match opcode {
            CREATE_BIT_FIELD => (index, 1),
            CREATE_BYTE_FIELD => (index * 8, 8),
            CREATE_WORD_FIELD => (index * 8, 16),
            CREATE_DWORD_FIELD => (index * 8, 32),
            CREATE_QWORD_FIELD => (index * 8, 64),
            _ => (index, self.integer_operand(frame, stream)?),
        };
        let name = stream.name_string()?;
        let field = BufferField {
            source,
            bit_offset,
            bit_length,
        };
        self.create(frame, &name, Object::BufferField(field))?;
        Ok(())
    }

    fn synchronize(&mut self, target: &Target, opcode: u16) -> Result<(), Error> {
        let Target::Node(node) = target else {
            return Err(Error::WrongType);
        };
        match (self.namespace.get_mut(*node), opcode) {
            (Some(Object::Event { signals }), SIGNAL) => *signals = signals.saturating_add(1),
            (Some(Object::Event { signals }), RESET) => *signals = 0,
            (Some(Object::Mutex { held, .. }), RELEASE) => *held = held.saturating_sub(1),
            _ => return Err(Error::WrongType),
        }
        Ok(())
    }

    /// A `DataRefObject` or a package element: a name stays a name.
    pub(crate) fn data_reference(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
    ) -> Result<Object, Error> {
        if !stream.at_name() {
            return self.operand(frame, stream);
        }
        let name = stream.name_string()?;
        let path = match self.namespace.search(&name, &frame.scope) {
            Some(found) => self.namespace.path_of(found).cloned(),
            None => name.resolve(&frame.scope),
        };
        path.map(Object::NameReference).ok_or(Error::NameNotFound)
    }

    /// The value a name gives as an operand: a method's result, called with as many operands as
    /// it takes; a field's contents; any other object as it is.
    pub(crate) fn name_value(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
        name: &NameString,
    ) -> Result<Object, Error> {
        let id = self
            .namespace
            .search(name, &frame.scope)
            .ok_or(Error::NameNotFound)?;
        let count = match self.namespace.get(id) {
            Some(Object::Method(method)) => usize::from(method.arguments),
            Some(Object::NativeMethod(NativeMethod::OperatingSystemInterface)) => 1,
            Some(Object::Field(_) | Object::BufferField(_)) => {
                return self.read_target(frame, &Target::Node(id));
            }
            Some(object) => return Ok(object.clone()),
            None => return Err(Error::NameNotFound),
        };
        let mut arguments = Vec::with_capacity(count);
        for _ in 0..count {
            arguments.push(self.operand(frame, stream)?);
        }
        self.evaluate_node(id, arguments)
    }

    pub(crate) fn operand(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
    ) -> Result<Object, Error> {
        if stream.at_name() {
            let name = stream.name_string()?;
            return self.name_value(frame, stream, &name);
        }
        let opcode = opcode(stream)?;
        self.operand_with(frame, stream, opcode)
    }

    pub(crate) fn integer_operand(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
    ) -> Result<u64, Error> {
        let value = self.operand(frame, stream)?;
        self.to_integer(&value)
    }

    pub(crate) fn string_operand(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
    ) -> Result<Vec<u8>, Error> {
        let value = self.operand(frame, stream)?;
        self.to_string_implicit(&value)
    }

    /// Where a `SuperName` or `Target` points: a local, an argument, `Debug`, a named object,
    /// nothing (the null name), or what a reference-giving expression gives.
    pub(crate) fn super_name(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
    ) -> Result<Target, Error> {
        self.super_name_if_present(frame, stream)?
            .ok_or(Error::NameNotFound)
    }

    /// [`Self::super_name`], with `None` for a name nothing answers to (`CondRefOf`).
    pub(crate) fn super_name_if_present(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
    ) -> Result<Option<Target>, Error> {
        if stream.at_name() {
            let name = stream.name_string()?;
            let Some(id) = self.namespace.search(&name, &frame.scope) else {
                return Ok(None);
            };
            if matches!(self.namespace.get(id), Some(Object::Method(_))) {
                let given = self.name_value_of(frame, stream, id)?;
                return Ok(Some(Self::target_of(given)));
            }
            return Ok(Some(Target::Node(id)));
        }
        let opcode = opcode(stream)?;
        let target = match opcode {
            ZERO => Target::Null,
            LOCAL_0..=LOCAL_7 => Target::Local((opcode - LOCAL_0) as u8),
            ARG_0..=ARG_6 => Target::Argument((opcode - ARG_0) as u8),
            DEBUG => Target::Debug,
            other => {
                let given = self.operand_with(frame, stream, other)?;
                Self::target_of(given)
            }
        };
        Ok(Some(target))
    }

    fn name_value_of(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
        id: NodeId,
    ) -> Result<Object, Error> {
        let count = match self.namespace.get(id) {
            Some(Object::Method(method)) => usize::from(method.arguments),
            _ => 0,
        };
        let mut arguments = Vec::with_capacity(count);
        for _ in 0..count {
            arguments.push(self.operand(frame, stream)?);
        }
        self.evaluate_node(id, arguments)
    }

    fn target_of(given: Object) -> Target {
        match given {
            Object::Reference(target) => target,
            other => Target::Temporary(alloc::boxed::Box::new(other)),
        }
    }

    /// The buffer, package or string an `Index` or `CreateField` works on: a location when it
    /// is written as one, so changes reach it, else the value the operand gives.
    pub(crate) fn container(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
    ) -> Result<Target, Error> {
        let lead = stream.peek().ok_or(Error::UnexpectedEnd)?;
        let is_location = stream.at_name() || (LOCAL_0..=ARG_6).contains(&u16::from(lead));
        if is_location {
            return self.super_name(frame, stream);
        }
        let value = self.operand(frame, stream)?;
        Ok(Self::target_of(value))
    }
}
