//! Walking every node of a device tree for device discovery (M5), allocating nothing: each node
//! comes with what a bus needs to describe it as a device: its `compatible` strings, its `reg`
//! with the parent's cell counts, its interrupts and the controller they go to (inherited
//! through `interrupt-parent`), whether it is enabled, and its DMA coherence.

use super::{Error, Fdt, Token, be32, cells_value};

/// Node levels the walker keeps context for; deeper nodes are skipped (QEMU's trees nest four
/// levels at most).
pub const MAX_NODE_DEPTH: usize = 16;

/// The most cells an interrupt specifier may have here (GICv3 with PPI partitions uses 4).
pub const MAX_INTERRUPT_CELLS: usize = 4;

/// One node of the tree, with the context its parent gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node<'a> {
    /// The node's name with its unit address (`virtio_mmio@a000000`); empty for the root.
    pub name: &'a [u8],
    /// 0 for the root, 1 for its children, ...
    pub depth: usize,
    /// The raw `compatible` property: NUL-terminated strings, most specific first.
    pub compatible: &'a [u8],
    /// The raw `reg` property, decoded with [`Self::address_cells`] and [`Self::size_cells`].
    pub reg: &'a [u8],
    /// The parent's `#address-cells` (2 when absent).
    pub address_cells: u32,
    /// The parent's `#size-cells` (1 when absent).
    pub size_cells: u32,
    /// Whether `reg` holds CPU physical addresses: every ancestor below the root has an empty
    /// `ranges` (an identity mapping). Addresses behind a non-empty `ranges` (a PCI host's
    /// children, say) would need translation, which the walker does not do yet.
    pub cpu_addressable: bool,
    /// The raw `interrupts` property, in the cells of [`Self::interrupt_parent`].
    pub interrupts: &'a [u8],
    /// The raw `interrupts-extended` property: (controller phandle, cells...) entries.
    pub interrupts_extended: &'a [u8],
    /// The node's own `interrupt-parent`, else the nearest ancestor's.
    pub interrupt_parent: Option<u32>,
    /// The node's `phandle` (or legacy `linux,phandle`).
    pub phandle: Option<u32>,
    /// The node's own `#interrupt-cells`: set on interrupt controllers and nexus nodes.
    pub interrupt_cells: Option<u32>,
    /// `status` is absent, `"okay"` or `"ok"`.
    pub enabled: bool,
    /// The node has `dma-coherent`.
    pub dma_coherent: bool,
    /// The node has `dma-noncoherent`.
    pub dma_noncoherent: bool,
}

impl<'a> Node<'a> {
    fn new(name: &'a [u8], depth: usize, parent: &Level) -> Self {
        Self {
            name,
            depth,
            compatible: &[],
            reg: &[],
            address_cells: parent.address_cells,
            size_cells: parent.size_cells,
            cpu_addressable: parent.children_addressable,
            interrupts: &[],
            interrupts_extended: &[],
            interrupt_parent: parent.interrupt_parent,
            phandle: None,
            interrupt_cells: None,
            enabled: true,
            dma_coherent: false,
            dma_noncoherent: false,
        }
    }

    /// The `compatible` strings, most specific first.
    pub fn compatible_strings(&self) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.compatible
            .split(|&byte| byte == 0)
            .filter(|string| !string.is_empty())
    }

    /// Whether any `compatible` string is `wanted`.
    pub fn is_compatible(&self, wanted: &[u8]) -> bool {
        self.compatible_strings().any(|string| string == wanted)
    }

    /// The (address, size) pairs of `reg`; pairs that do not fit in 64 bits are skipped.
    pub fn reg_entries(&self) -> impl Iterator<Item = (u64, u64)> + 'a {
        let address_cells = self.address_cells as usize;
        let size_cells = self.size_cells as usize;
        let entry = (address_cells + size_cells) * 4;
        let pairs = if entry == 0 || address_cells > 4 || size_cells > 4 {
            [].chunks_exact(1)
        } else {
            self.reg.chunks_exact(entry)
        };
        pairs.filter_map(move |pair| {
            let (address, size) = pair.split_at(address_cells * 4);
            Some((cells_value(address)?, cells_value(size)?))
        })
    }

    /// The node's interrupts, from `interrupts-extended` if it has one, else from `interrupts`
    /// with [`Self::interrupt_parent`]. `cells_of` gives a controller's `#interrupt-cells` by
    /// phandle; a controller it does not know, or a specifier wider than
    /// [`MAX_INTERRUPT_CELLS`], ends the list.
    pub fn interrupt_specifiers<F: Fn(u32) -> Option<u32>>(
        &self,
        cells_of: F,
    ) -> InterruptSpecifiers<'a, F> {
        let extended = !self.interrupts_extended.is_empty();
        InterruptSpecifiers {
            raw: if extended {
                self.interrupts_extended
            } else {
                self.interrupts
            },
            parent: if extended {
                None
            } else {
                self.interrupt_parent
            },
            cells_of,
        }
    }
}

/// The interrupts of a [`Node`], from [`Node::interrupt_specifiers`].
#[derive(Debug, Clone)]
pub struct InterruptSpecifiers<'a, F> {
    raw: &'a [u8],
    parent: Option<u32>,
    cells_of: F,
}

impl<F: Fn(u32) -> Option<u32>> Iterator for InterruptSpecifiers<'_, F> {
    type Item = InterruptSpecifier;

    fn next(&mut self) -> Option<InterruptSpecifier> {
        let (controller, cells_at) = match self.parent {
            Some(parent) => (parent, 0),
            None => (be32(self.raw, 0)?, 4),
        };
        let specifier = (self.cells_of)(controller)
            .map(|count| count as usize)
            .filter(|&count| count <= MAX_INTERRUPT_CELLS)
            .and_then(|count| {
                let end = cells_at + count * 4;
                let mut cells = [0; MAX_INTERRUPT_CELLS];
                for (index, cell) in cells.iter_mut().take(count).enumerate() {
                    *cell = be32(self.raw, cells_at + index * 4)?;
                }
                Some((end, cells, count))
            });
        let Some((end, cells, count)) = specifier else {
            self.raw = &[];
            return None;
        };
        self.raw = &self.raw[end..];
        Some(InterruptSpecifier {
            controller,
            cells,
            count: count as u8,
        })
    }
}

/// One interrupt of a node: the controller's phandle and the specifier's cells, which only
/// that controller's binding gives a meaning (GICv2: type, number, flags; PLIC: the source).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptSpecifier {
    pub controller: u32,
    pub cells: [u32; MAX_INTERRUPT_CELLS],
    /// How many of `cells` the specifier has.
    pub count: u8,
}

impl InterruptSpecifier {
    /// The specifier's cells.
    pub fn cells(&self) -> &[u32] {
        &self.cells[..usize::from(self.count)]
    }
}

#[derive(Debug, Clone, Copy)]
struct Level {
    address_cells: u32,
    size_cells: u32,
    interrupt_parent: Option<u32>,
    children_addressable: bool,
}

const ROOT_CONTEXT: Level = Level {
    address_cells: 2,
    size_cells: 1,
    interrupt_parent: None,
    children_addressable: true,
};

impl<'a> Fdt<'a> {
    /// Every node of the tree, parents before their children, each with its parent's context
    /// (see [`Node`]). A malformed or truncated tree ends the walk with the error, after the
    /// nodes read before it. Nodes deeper than [`MAX_NODE_DEPTH`] are skipped.
    pub fn nodes(&self) -> impl Iterator<Item = Result<Node<'a>, Error>> + 'a {
        let mut tokens = self.tokens();
        let mut levels = [ROOT_CONTEXT; MAX_NODE_DEPTH + 1];
        let mut pending: Option<Node<'a>> = None;
        let mut has_ranges = false;
        let mut empty_ranges = false;
        let mut depth = 0usize;
        let mut ended = false;
        core::iter::from_fn(move || {
            if ended {
                return None;
            }
            loop {
                let token = match tokens.next() {
                    Some(Ok(token)) => token,
                    Some(Err(error)) => {
                        ended = true;
                        return Some(Err(error));
                    }
                    None => {
                        ended = true;
                        return (depth != 0).then_some(Err(Error::Malformed));
                    }
                };
                match token {
                    Token::BeginNode(name) => {
                        let parent_ready = pending.take().inspect(|node| {
                            let level = &mut levels[node.depth + 1];
                            level.interrupt_parent = node.interrupt_parent;
                            level.children_addressable = node.depth == 0
                                || (node.cpu_addressable && has_ranges && empty_ranges);
                        });
                        if depth < MAX_NODE_DEPTH {
                            let parent = levels[depth];
                            levels[depth + 1] = Level {
                                address_cells: 2,
                                size_cells: 1,
                                ..parent
                            };
                            pending = Some(Node::new(name, depth, &parent));
                        }
                        (has_ranges, empty_ranges) = (false, false);
                        depth += 1;
                        if parent_ready.is_some() {
                            return parent_ready.map(Ok);
                        }
                    }
                    Token::EndNode => {
                        let Some(below) = depth.checked_sub(1) else {
                            ended = true;
                            return Some(Err(Error::Malformed));
                        };
                        depth = below;
                        if let Some(node) = pending.take() {
                            return Some(Ok(node));
                        }
                    }
                    Token::Property(name, value) => {
                        if depth == 0 {
                            ended = true;
                            return Some(Err(Error::Malformed));
                        }
                        if depth > MAX_NODE_DEPTH {
                            continue;
                        }
                        let own = &mut levels[depth];
                        if let Some(node) = pending.as_mut() {
                            apply(node, own, name, value);
                            if name == b"ranges" {
                                (has_ranges, empty_ranges) = (true, value.is_empty());
                            }
                        }
                    }
                }
            }
        })
    }
}

#[derive(Clone, Copy)]
enum Property {
    Compatible,
    Reg,
    Interrupts,
    InterruptsExtended,
    InterruptParent,
    Phandle,
    InterruptCells,
    AddressCells,
    SizeCells,
    Status,
    DmaCoherent,
    DmaNoncoherent,
}

fn property(name: &[u8]) -> Option<Property> {
    use Property::*;
    let candidates: &[(&[u8], Property)] = match name.len() {
        3 => &[(b"reg", Reg)],
        6 => &[(b"status", Status)],
        7 => &[(b"phandle", Phandle)],
        10 => &[(b"compatible", Compatible), (b"interrupts", Interrupts)],
        11 => &[(b"#size-cells", SizeCells)],
        12 => &[(b"dma-coherent", DmaCoherent)],
        13 => &[(b"linux,phandle", Phandle)],
        14 => &[(b"#address-cells", AddressCells)],
        15 => &[(b"dma-noncoherent", DmaNoncoherent)],
        16 => &[
            (b"interrupt-parent", InterruptParent),
            (b"#interrupt-cells", InterruptCells),
        ],
        19 => &[(b"interrupts-extended", InterruptsExtended)],
        _ => return None,
    };
    candidates
        .iter()
        .find(|(known, _)| *known == name)
        .map(|&(_, property)| property)
}

fn apply<'a>(node: &mut Node<'a>, own: &mut Level, name: &[u8], value: &'a [u8]) {
    let Some(property) = property(name) else {
        return;
    };
    let cell = be32(value, 0);
    match property {
        Property::Compatible => node.compatible = value,
        Property::Reg => node.reg = value,
        Property::Interrupts => node.interrupts = value,
        Property::InterruptsExtended => node.interrupts_extended = value,
        Property::InterruptParent => node.interrupt_parent = cell,
        Property::Phandle => node.phandle = cell,
        Property::InterruptCells => node.interrupt_cells = cell,
        Property::AddressCells => own.address_cells = cell.unwrap_or(2),
        Property::SizeCells => own.size_cells = cell.unwrap_or(1),
        Property::Status => node.enabled = value == b"okay\0" || value == b"ok\0",
        Property::DmaCoherent => node.dma_coherent = true,
        Property::DmaNoncoherent => node.dma_noncoherent = true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fdt::tests::Builder;

    const QEMU_AARCH64_VIRT: &[u8] = include_bytes!("testdata/qemu-aarch64-virt.dtb");
    const QEMU_RISCV64_VIRT: &[u8] = include_bytes!("testdata/qemu-riscv64-virt.dtb");

    fn nodes_of(blob: &[u8]) -> Vec<Node<'_>> {
        Fdt::new(blob)
            .unwrap()
            .nodes()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn controllers<'a>(nodes: &[Node<'a>]) -> impl Fn(u32) -> Option<u32> + 'a {
        let known: Vec<(u32, u32)> = nodes
            .iter()
            .filter_map(|node| Some((node.phandle?, node.interrupt_cells?)))
            .collect();
        move |phandle| {
            known
                .iter()
                .find_map(|&(p, cells)| (p == phandle).then_some(cells))
        }
    }

    #[test]
    fn qemu_aarch64_virt_lists_its_virtio_mmio_slots_with_gic_interrupts() {
        let nodes = nodes_of(QEMU_AARCH64_VIRT);
        let cells_of = controllers(&nodes);
        let slots: Vec<_> = nodes
            .iter()
            .filter(|node| node.is_compatible(b"virtio,mmio"))
            .collect();
        assert_eq!(slots.len(), 32);
        let first = slots
            .iter()
            .find(|node| node.name == b"virtio_mmio@a000000")
            .unwrap();
        assert_eq!(first.depth, 1);
        assert!(first.enabled && first.dma_coherent && first.cpu_addressable);
        assert_eq!(
            first.reg_entries().collect::<Vec<_>>(),
            [(0xa00_0000, 0x200)]
        );
        let interrupts: Vec<_> = first.interrupt_specifiers(&cells_of).collect();
        assert_eq!(interrupts.len(), 1);
        assert_eq!(interrupts[0].cells(), [0, 0x10, 1]);
        let gic = nodes
            .iter()
            .find(|node| node.is_compatible(b"arm,cortex-a15-gic"))
            .unwrap();
        assert_eq!(Some(interrupts[0].controller), gic.phandle);
        assert_eq!(gic.interrupt_cells, Some(3));
        let mut addresses: Vec<_> = slots
            .iter()
            .flat_map(|node| node.reg_entries())
            .map(|(address, _)| address)
            .collect();
        addresses.sort_unstable();
        assert_eq!(
            addresses,
            (0..32)
                .map(|slot| 0xa00_0000 + slot * 0x200)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn qemu_riscv64_virt_nests_its_devices_in_soc_with_plic_sources() {
        let nodes = nodes_of(QEMU_RISCV64_VIRT);
        let cells_of = controllers(&nodes);
        let plic = nodes
            .iter()
            .find(|node| node.is_compatible(b"riscv,plic0"))
            .unwrap();
        let mut slots: Vec<_> = nodes
            .iter()
            .filter(|node| node.is_compatible(b"virtio,mmio"))
            .map(|node| {
                let (address, size) = node.reg_entries().next().unwrap();
                let interrupt = node.interrupt_specifiers(&cells_of).next().unwrap();
                assert_eq!(node.depth, 2);
                assert!(node.cpu_addressable && !node.dma_coherent && !node.dma_noncoherent);
                assert_eq!(Some(interrupt.controller), plic.phandle);
                (address, size, interrupt.cells().to_vec())
            })
            .collect();
        slots.sort_unstable();
        assert_eq!(slots.len(), 8);
        for (slot, (address, size, cells)) in slots.into_iter().enumerate() {
            assert_eq!(address, 0x1000_1000 + slot as u64 * 0x1000);
            assert_eq!(size, 0x1000);
            assert_eq!(cells, [slot as u32 + 1]);
        }
        let plic_contexts: Vec<_> = plic.interrupt_specifiers(&cells_of).collect();
        assert_eq!(plic_contexts.len(), 8);
        assert!(plic_contexts.iter().all(|context| context.count == 1));
        assert_eq!(plic_contexts[1].cells(), [9]);
    }

    #[test]
    fn every_node_comes_once_parents_first() {
        for blob in [QEMU_AARCH64_VIRT, QEMU_RISCV64_VIRT] {
            let nodes = nodes_of(blob);
            assert_eq!(nodes[0].name, b"");
            assert_eq!(nodes[0].depth, 0);
            for pair in nodes.windows(2) {
                assert!(pair[1].depth <= pair[0].depth + 1);
            }
            let mut names: Vec<_> = nodes.iter().map(|node| (node.depth, node.name)).collect();
            let total = names.len();
            names.sort_unstable();
            names.dedup();
            assert!(names.len() >= total - 8);
        }
    }

    fn tree_with_nesting() -> Vec<u8> {
        let mut b = Builder::default();
        b.begin("")
            .cells("#address-cells", &[2])
            .cells("#size-cells", &[2])
            .cells("interrupt-parent", &[1])
            .begin("intc@0")
            .cells("phandle", &[1])
            .cells("#interrupt-cells", &[2])
            .prop("interrupt-controller", &[])
            .end()
            .begin("other-intc")
            .cells("phandle", &[7])
            .cells("#interrupt-cells", &[1])
            .end()
            .begin("bus")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[1])
            .prop("ranges", &[])
            .begin("dev@1000")
            .prop("compatible", b"vendor,specific\0vendor,generic\0")
            .cells("reg", &[0x1000, 0x100, 0x2000, 0x10])
            .cells("interrupts", &[5, 4, 6, 1])
            .prop("dma-noncoherent", &[])
            .end()
            .begin("off@3000")
            .prop("status", b"disabled\0")
            .cells("reg", &[0x3000, 0x100])
            .cells("interrupts-extended", &[7, 9, 1, 3, 4])
            .end()
            .begin("bridge")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[1])
            .cells("ranges", &[0, 0x8000, 0x1000])
            .begin("child@0")
            .cells("reg", &[0, 0x10])
            .end()
            .end()
            .end()
            .begin("no-ranges")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[0])
            .begin("i2c-device@50")
            .cells("reg", &[0x50])
            .end()
            .end()
            .end();
        b.build()
    }

    #[test]
    fn reg_uses_the_parents_cells_and_interrupts_their_controllers_cells() {
        let blob = tree_with_nesting();
        let nodes = nodes_of(&blob);
        let cells_of = controllers(&nodes);
        let device = nodes.iter().find(|node| node.name == b"dev@1000").unwrap();
        assert_eq!(
            device.compatible_strings().collect::<Vec<_>>(),
            [&b"vendor,specific"[..], b"vendor,generic"]
        );
        assert_eq!(
            device.reg_entries().collect::<Vec<_>>(),
            [(0x1000, 0x100), (0x2000, 0x10)]
        );
        assert!(device.cpu_addressable && device.dma_noncoherent && device.enabled);
        let interrupts: Vec<_> = device.interrupt_specifiers(&cells_of).collect();
        assert_eq!(interrupts.len(), 2);
        assert!(interrupts.iter().all(|i| i.controller == 1));
        assert_eq!(interrupts[1].cells(), [6, 1]);
    }

    #[test]
    fn interrupts_extended_names_a_controller_per_entry_and_status_disables() {
        let blob = tree_with_nesting();
        let nodes = nodes_of(&blob);
        let cells_of = controllers(&nodes);
        let off = nodes.iter().find(|node| node.name == b"off@3000").unwrap();
        assert!(!off.enabled);
        let interrupts: Vec<_> = off.interrupt_specifiers(&cells_of).collect();
        assert_eq!(
            interrupts
                .iter()
                .map(|i| (i.controller, i.cells().to_vec()))
                .collect::<Vec<_>>(),
            [(7, vec![9]), (1, vec![3, 4])]
        );
    }

    #[test]
    fn addresses_behind_a_translating_or_missing_ranges_are_not_cpu_addresses() {
        let blob = tree_with_nesting();
        let nodes = nodes_of(&blob);
        let child = nodes.iter().find(|node| node.name == b"child@0").unwrap();
        assert!(!child.cpu_addressable);
        let i2c = nodes
            .iter()
            .find(|node| node.name == b"i2c-device@50")
            .unwrap();
        assert!(!i2c.cpu_addressable);
        assert_eq!(i2c.reg_entries().collect::<Vec<_>>(), [(0x50, 0)]);
        let bus = nodes.iter().find(|node| node.name == b"bus").unwrap();
        assert!(bus.cpu_addressable);
    }

    #[test]
    fn an_unknown_controller_or_a_short_specifier_ends_the_interrupts() {
        let blob = tree_with_nesting();
        let nodes = nodes_of(&blob);
        let device = nodes.iter().find(|node| node.name == b"dev@1000").unwrap();
        assert_eq!(device.interrupt_specifiers(|_| None).count(), 0);
        let short = Node {
            interrupts: &device.interrupts[..12],
            ..*device
        };
        assert_eq!(short.interrupt_specifiers(|_| Some(2)).count(), 1);
        assert_eq!(device.interrupt_specifiers(|_| Some(9)).count(), 0);
    }

    #[test]
    fn truncated_and_corrupt_trees_end_with_an_error_never_a_panic() {
        let blob = tree_with_nesting();
        let fdt = Fdt::new(&blob).unwrap();
        for cut in 0..fdt.structure.len() {
            let short = Fdt {
                structure: &fdt.structure[..cut],
                ..fdt
            };
            assert!(short.nodes().any(|node| node.is_err()), "cut at {cut}");
        }
        let structure = be32(&blob, 8).unwrap() as usize;
        for at in structure..structure + fdt.structure.len() {
            for value in [0u8, 1, 2, 3, 9, 0xff] {
                let mut corrupt = blob.clone();
                corrupt[at] = value;
                if let Ok(fdt) = Fdt::new(&corrupt) {
                    assert!(fdt.nodes().count() < 64);
                }
            }
        }
        let aarch64 = &QEMU_AARCH64_VIRT[..QEMU_AARCH64_VIRT.len() / 2];
        assert!(Fdt::new(aarch64).is_err());
    }

    #[test]
    fn nodes_deeper_than_the_walker_keeps_are_skipped() {
        let mut b = Builder::default();
        b.begin("");
        for _ in 0..MAX_NODE_DEPTH + 4 {
            b.begin("n").prop("compatible", b"deep\0");
        }
        for _ in 0..MAX_NODE_DEPTH + 4 {
            b.end();
        }
        b.begin("after")
            .prop("compatible", b"shallow\0")
            .end()
            .end();
        let blob = b.build();
        let nodes = nodes_of(&blob);
        assert_eq!(nodes.len(), MAX_NODE_DEPTH + 1);
        assert_eq!(nodes.last().unwrap().name, b"after");
        assert_eq!(nodes.last().unwrap().depth, 1);
    }
}
