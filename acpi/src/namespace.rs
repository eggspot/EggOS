//! The ACPI namespace (ACPI 6.5, 5.3): every object by its absolute path, the scopes it starts
//! with, and the search rules a single-segment name follows.

use crate::Error;
use crate::name::{NameSeg, NameString, Path};
use crate::object::{NativeMethod, Object};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// A node of the namespace: an index that stays valid while the node lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub(crate) usize);

/// What `\_REV` holds: the revision of the ACPI specification the interpreter follows.
pub const REVISION: u64 = 2;
const OPERATING_SYSTEM_NAME: &[u8] = b"Microsoft Windows NT";

#[derive(Debug)]
struct Node {
    path: Path,
    object: Object,
}

/// Every object, by path.
#[derive(Debug)]
pub struct Namespace {
    nodes: Vec<Option<Node>>,
    by_path: BTreeMap<Path, NodeId>,
}

impl Default for Namespace {
    fn default() -> Self {
        Self::new()
    }
}

impl Namespace {
    /// The namespace before any table loads: the root, its predefined scopes (`\_GPE`, `\_PR`,
    /// `\_SB`, `\_SI`, `\_TZ`) and objects (`\_OSI`, `\_OS`, `\_REV`, `\_GL`).
    pub fn new() -> Self {
        let mut namespace = Self {
            nodes: Vec::new(),
            by_path: BTreeMap::new(),
        };
        let root = Path::root();
        let predefined = [
            (b"_GPE", Object::Scope),
            (b"_PR_", Object::Scope),
            (b"_SB_", Object::Device),
            (b"_SI_", Object::Scope),
            (b"_TZ_", Object::Device),
            (
                b"_OSI",
                Object::NativeMethod(NativeMethod::OperatingSystemInterface),
            ),
            (b"_OS_", Object::String(OPERATING_SYSTEM_NAME.to_vec())),
            (b"_REV", Object::Integer(REVISION)),
            (
                b"_GL_",
                Object::Mutex {
                    sync_level: 0,
                    held: 0,
                },
            ),
        ];
        let _ = namespace.add(root.clone(), Object::Scope);
        for (name, object) in predefined {
            let _ = namespace.add(root.child(NameSeg::new(name)), object);
        }
        namespace
    }

    /// Adds `object` at `path`, whose parent must exist: its node. An existing node is refused,
    /// unless both are scopes of the same kind, which a later table may name again.
    pub fn add(&mut self, path: Path, object: Object) -> Result<NodeId, Error> {
        if let Some(&existing) = self.by_path.get(&path) {
            let same_scope = object.is_scope()
                && self.get(existing).is_some_and(|held| {
                    core::mem::discriminant(held) == core::mem::discriminant(&object)
                });
            return match same_scope {
                true => Ok(existing),
                false => Err(Error::AlreadyExists),
            };
        }
        if let Some(parent) = path.parent()
            && !self.by_path.contains_key(&parent)
        {
            return Err(Error::NameNotFound);
        }
        let id = NodeId(self.nodes.len());
        self.nodes.push(Some(Node {
            path: path.clone(),
            object,
        }));
        self.by_path.insert(path, id);
        Ok(id)
    }

    /// Makes `alias` name the node `target` too.
    pub fn add_alias(&mut self, alias: Path, target: NodeId) -> Result<(), Error> {
        if self.by_path.contains_key(&alias) {
            return Err(Error::AlreadyExists);
        }
        self.by_path.insert(alias, target);
        Ok(())
    }

    /// Removes the node `id` and every name that leads to it: a method's temporary objects when
    /// it returns.
    pub fn remove(&mut self, id: NodeId) {
        if let Some(slot) = self.nodes.get_mut(id.0) {
            *slot = None;
        }
        self.by_path.retain(|_, node| *node != id);
    }

    pub fn get(&self, id: NodeId) -> Option<&Object> {
        self.nodes.get(id.0)?.as_ref().map(|node| &node.object)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Object> {
        self.nodes
            .get_mut(id.0)?
            .as_mut()
            .map(|node| &mut node.object)
    }

    /// The path the node was made at.
    pub fn path_of(&self, id: NodeId) -> Option<&Path> {
        self.nodes.get(id.0)?.as_ref().map(|node| &node.path)
    }

    pub fn find(&self, path: &Path) -> Option<NodeId> {
        self.by_path.get(path).copied()
    }

    /// The node `name` names from `scope`: a single segment with no prefix is looked for in
    /// `scope`, then in each scope above it up to the root (5.3); any other name only where it
    /// points.
    pub fn search(&self, name: &NameString, scope: &Path) -> Option<NodeId> {
        if !name.searches() {
            return self.find(&name.resolve(scope)?);
        }
        let segment = name.segments[0];
        let mut at = scope.clone();
        loop {
            if let Some(found) = self.find(&at.child(segment)) {
                return Some(found);
            }
            at = at.parent()?;
        }
    }

    /// The nodes right below `path`, in path order.
    pub fn children(&self, path: &Path) -> Vec<NodeId> {
        let depth = path.depth() + 1;
        self.by_path
            .range(path.clone()..)
            .skip_while(|(candidate, _)| *candidate == path)
            .take_while(|(candidate, _)| candidate.starts_with(path))
            .filter(|(candidate, _)| candidate.depth() == depth)
            .map(|(_, &id)| id)
            .collect()
    }

    /// Every name and its node, in path order.
    pub fn iter(&self) -> impl Iterator<Item = (&Path, NodeId)> + '_ {
        self.by_path.iter().map(|(path, &id)| (path, id))
    }
}
