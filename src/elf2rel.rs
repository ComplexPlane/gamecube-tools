use std::cmp::Ordering;
use std::collections::HashMap;

use anyhow::{anyhow, Context};
use anyhow::{bail, ensure};
use num_enum::{IntoPrimitive, TryFromPrimitive};
use object::{
    Architecture, BinaryFormat, Endianness, Object, ObjectSection, ObjectSymbol, RelocationFlags,
    RelocationTarget, SectionIndex, SectionKind, SymbolSection,
};
use zerocopy::{big_endian, Immutable, IntoBytes, KnownLayout};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, TryFromPrimitive, IntoPrimitive)]
#[repr(u8)]
pub enum RelVersion {
    V1 = 1,
    V2 = 2,
    V3 = 3,
}

#[derive(Default, Immutable, KnownLayout, IntoBytes)]
#[repr(C)]
struct ModuleHeader {
    id: big_endian::U32,
    prev_link: big_endian::U32,
    next_link: big_endian::U32,
    section_count: big_endian::U32,
    section_info_offset: big_endian::U32,
    name_offset: big_endian::U32,
    name_size: big_endian::U32,
    version: big_endian::U32,

    total_bss_size: big_endian::U32,
    relocation_offset: big_endian::U32,
    import_info_offset: big_endian::U32,
    import_info_size: big_endian::U32,
    prolog_section: u8,
    epilog_section: u8,
    unresolved_section: u8,
    pad: u8,
    prolog_offset: big_endian::U32,
    epilog_offset: big_endian::U32,
    unresolved_offset: big_endian::U32,
}

#[derive(Default, Immutable, KnownLayout, IntoBytes)]
#[repr(C)]
struct ModuleV2HeaderAddendum {
    max_align: big_endian::U32,
    max_bss_align: big_endian::U32,
}

#[derive(Default, Immutable, KnownLayout, IntoBytes)]
#[repr(C)]
struct ModuleV3HeaderAddendum {
    fixed_data_size: big_endian::U32,
}

#[derive(Default, Immutable, KnownLayout, IntoBytes)]
#[repr(C)]
struct SectionInfo {
    offset: big_endian::U32,
    size: big_endian::U32,
}

#[derive(Default, Immutable, KnownLayout, IntoBytes)]
#[repr(C)]
struct ImportInfo {
    id: big_endian::U32,
    offset: big_endian::U32,
}

#[derive(Default, Immutable, KnownLayout, IntoBytes)]
#[repr(C)]
struct Relocation {
    offset: big_endian::U16,
    type_: u8,
    section: u8,
    addend: big_endian::U32,
}

#[derive(Debug, Clone, Copy, TryFromPrimitive, IntoPrimitive)]
#[repr(u8)]
enum RelocationType {
    PpcNone,
    PpcAddr32,
    PpcAddr24,
    PpcAddr16,
    PpcAddr16Lo,
    PpcAddr16Hi,
    PpcAddr16Ha,
    PpcAddr14,
    PpcAddr14BrTaken,
    PpcAddr14BrNkTaken,
    PpcRel24,
    PpcRel14,

    PpcRel32 = 26,

    DolphinNop = 201,
    DolphinSection,
    DolphinEnd,
}

#[derive(Debug)]
struct ElfRelocation {
    src_section: SectionIndex,
    src_offset: u32,
    dest_module: u32,
    dest_section: SectionIndex,
    addend: u32,
    type_: RelocationType,
}

struct SectionStats {
    total_bss_size: u32,
    max_align: u32,
    max_bss_align: u32,
    section_info_offset: u32,
    section_offsets: HashMap<SectionIndex, usize>,
}

struct RelocationStats {
    relocations_offset: u32,
    import_info_offset: u32,
    import_info_size: u32,
}

impl Ord for ElfRelocation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.dest_module
            .cmp(&other.dest_module)
            .then(self.src_section.0.cmp(&other.src_section.0))
            .then(self.src_offset.cmp(&other.src_offset))
    }
}

impl PartialOrd for ElfRelocation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ElfRelocation {
    fn eq(&self, other: &Self) -> bool {
        matches!(self.cmp(other), Ordering::Equal)
    }
}

impl Eq for ElfRelocation {}

const VALID_REL_SECTIONS: &[&str] = &[
    ".init", ".text", ".ctors", ".dtors", ".rodata", ".data", ".bss",
];

fn find_symbol<'a>(f: &'a object::File, name: &str) -> anyhow::Result<object::Symbol<'a, 'a>> {
    f.symbol_by_name(name)
        .ok_or_else(|| anyhow!("Could not find symbol in ELF: '{name}'"))
}

fn parse_symbol_map(buf: &[u8]) -> anyhow::Result<HashMap<&str, u32>> {
    let mut map = HashMap::new();
    let s = std::str::from_utf8(buf).context("Failed to parse symbol map as UTF-8")?;

    for (line_num, line) in s.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let (addr, name) = line
            .split_once(':')
            .ok_or_else(|| anyhow!("Invalid symbol mapping on line {}: {}", line_num + 1, line))?;
        if name.is_empty() {
            bail!("Empty symbol name on line {}", line_num + 1);
        }
        let addr = u32::from_str_radix(addr.trim(), 16).with_context(|| {
            format!("Failed to parse address on line {}: {}", line_num + 1, addr)
        })?;
        map.insert(name, addr);
    }

    Ok(map)
}

fn write_sections(elf: &object::File, rel: &mut Vec<u8>) -> anyhow::Result<SectionStats> {
    let section_info_offset = rel.len();
    // Write section infos first, before section offsets are determined
    for _ in elf.sections() {
        rel.extend_from_slice(SectionInfo::default().as_bytes());
    }

    // Track which offsets sections have been written to
    let mut section_offsets = HashMap::new();

    let mut section_info_buffer = Vec::new();
    let mut total_bss_size = 0;
    let mut max_align = 2;
    let mut max_bss_align = 2;
    for section in elf.sections() {
        let valid_section_name = VALID_REL_SECTIONS.iter().any(|cand_name| {
            section.name().map_or(false, |section_name| {
                &section_name == cand_name || section_name.starts_with(&format!("{cand_name}."))
            })
        });
        if valid_section_name {
            // Include this section
            if section.kind().is_bss() {
                max_bss_align = max_bss_align.max(section.align());
                let size = section.size();
                total_bss_size += size;

                let section_info = SectionInfo {
                    offset: 0.into(),
                    size: (size as u32).into(),
                };
                section_info_buffer.extend_from_slice(section_info.as_bytes());
            } else {
                // Update max alignment (minimum 2, low offset bit is used for exec flag)
                let align = section.align().max(2) as usize;
                max_align = max_align.max(align);

                // Write padding
                rel.resize(rel.len().next_multiple_of(align), 0);

                // Mark executable section in the offset
                let encoded_offset = if section.kind() == SectionKind::Text {
                    rel.len() | 1
                } else {
                    rel.len()
                };

                // Write section info
                let section_info = SectionInfo {
                    offset: (encoded_offset as u32).into(),
                    size: (section.size() as u32).into(),
                };
                section_info_buffer.extend_from_slice(section_info.as_bytes());

                // Write section data to main buffer
                section_offsets.insert(section.index(), rel.len());
                rel.extend_from_slice(section.data()?);
            }
        } else {
            // Remove this section
            let section_info = SectionInfo {
                offset: 0.into(),
                size: 0.into(),
            };
            section_info_buffer.extend_from_slice(section_info.as_bytes());
        }
    }

    // Fill in section info in main buffer
    let rel_section_info =
        &mut rel[section_info_offset..section_info_offset + section_info_buffer.len()];
    rel_section_info.copy_from_slice(&section_info_buffer);

    Ok(SectionStats {
        total_bss_size: total_bss_size as u32,
        max_align: max_align as u32,
        max_bss_align: max_bss_align as u32,
        section_info_offset: section_info_offset as u32,
        section_offsets,
    })
}

fn extract_relocations(
    elf: &object::File,
    symbol_map: &[u8],
    module_id: u32,
    section_offsets: &HashMap<SectionIndex, usize>,
) -> anyhow::Result<Vec<ElfRelocation>> {
    let mut relocations = Vec::new();

    let symbol_map = parse_symbol_map(symbol_map).context("Failed to parse symbol map")?;

    for src_section in elf.sections() {
        // Don't include relocations for unwritten sections
        if !section_offsets.contains_key(&src_section.index()) {
            continue;
        }

        for (src_offset, relocation) in src_section.relocations() {
            let RelocationTarget::Symbol(symbol_idx) = relocation.target() else {
                bail!("Unsupported relocation target");
            };
            let dest_symbol = elf.symbol_by_index(symbol_idx).unwrap();

            let RelocationFlags::Elf { r_type } = relocation.flags() else {
                panic!("Expected ELF relocation flags");
            };
            let type_ = RelocationType::try_from(r_type as u8)
                .map_err(|_| anyhow!("Unsupported ELF relocation type: {r_type}"))?;

            match dest_symbol.section() {
                SymbolSection::Section(dest_section_idx) => {
                    // Relocation against self
                    relocations.push(ElfRelocation {
                        src_section: src_section.index(),
                        src_offset: src_offset as u32,
                        dest_module: module_id,
                        dest_section: SectionIndex(dest_section_idx.0),
                        addend: (dest_symbol.address() as i64 + relocation.addend()) as u32,
                        type_,
                    });
                }
                SymbolSection::Undefined => {
                    // Relocation against external symbol
                    let symbol_name = dest_symbol.name()?;
                    let dest_symbol_addr = *symbol_map.get(&symbol_name).ok_or_else(|| {
                        anyhow!("External symbol '{}' not found in symbol map", symbol_name)
                    })?;
                    relocations.push(ElfRelocation {
                        src_section: src_section.index(),
                        src_offset: src_offset as u32,
                        dest_module: 0,
                        dest_section: SectionIndex(0),
                        addend: (dest_symbol_addr as i64 + relocation.addend()) as u32,
                        type_,
                    });
                }
                section => bail!("Unsupported symbol section: {:?}", section),
            }
        }
    }

    relocations.sort_unstable();

    Ok(relocations)
}

fn statically_apply_relocation(
    rel: &mut [u8],
    section_offsets: &HashMap<SectionIndex, usize>,
    relocation: &ElfRelocation,
) {
    let src_offset =
        *section_offsets.get(&relocation.src_section).unwrap() + relocation.src_offset as usize;
    let delta = *section_offsets.get(&relocation.dest_section).unwrap() as i32
        + relocation.addend as i32
        - src_offset as i32;

    let data_slice = &mut rel[src_offset..src_offset + 4];
    let mut data = i32::from_be_bytes(data_slice.try_into().unwrap());
    match relocation.type_ {
        RelocationType::PpcRel24 => {
            data |= delta & 0x03FFFFFC;
        }
        RelocationType::PpcRel32 => {
            data = delta;
        }
        _ => panic!("Unexpected relocation type"),
    }
    data_slice.copy_from_slice(&data.to_be_bytes());
}

fn write_relocations(
    rel: &mut Vec<u8>,
    elf_relocations: &[ElfRelocation],
    module_id: u32,
    section_offsets: &HashMap<SectionIndex, usize>,
) -> anyhow::Result<RelocationStats> {
    // Count modules
    let mut import_count = 0;
    let mut last_module_id = None;
    for relocation in elf_relocations {
        if Some(relocation.dest_module) != last_module_id {
            import_count += 1;
            last_module_id = Some(relocation.dest_module);
        }
    }

    // Write padding for imports
    rel.resize(rel.len().next_multiple_of(8), 0);

    // Write dummy imports
    let import_info_offset = rel.len();
    for _ in 0..import_count {
        rel.extend_from_slice(ImportInfo::default().as_bytes());
    }

    // Write out relocations
    let relocation_offset = rel.len();

    let mut import_info_buffer = Vec::new();
    let mut current_module_id = None;
    let mut current_section_index = None;
    let mut current_offset = 0;

    for relocation in elf_relocations {
        // Resolve early if possible
        if relocation.dest_module == module_id
            && matches!(
                relocation.type_,
                RelocationType::PpcRel24 | RelocationType::PpcRel32
            )
        {
            statically_apply_relocation(rel, section_offsets, relocation);
            continue;
        }

        // Change module if necessary
        if current_module_id != Some(relocation.dest_module) {
            // Not first module?
            if current_module_id.is_some() {
                let r = Relocation {
                    offset: 0.into(),
                    type_: u8::from(RelocationType::DolphinEnd),
                    section: 0,
                    addend: 0.into(),
                };
                rel.extend_from_slice(r.as_bytes());
            }

            current_module_id = Some(relocation.dest_module);
            current_section_index = None;
            let import = ImportInfo {
                id: relocation.dest_module.into(),
                offset: (rel.len() as u32).into(),
            };
            import_info_buffer.extend_from_slice(import.as_bytes());
        }

        // Change section if necessary
        if current_section_index != Some(relocation.src_section) {
            current_section_index = Some(relocation.src_section);
            current_offset = 0;
            let r = Relocation {
                offset: 0.into(),
                type_: u8::from(RelocationType::DolphinSection),
                section: relocation.src_section.0 as u8,
                addend: 0.into(),
            };
            rel.extend_from_slice(r.as_bytes());
        }

        // Get into range of target
        const MAX_OFFSET_DELTA: u16 = 0xFFFF;
        let mut target_delta = relocation.src_offset - current_offset;
        while target_delta > MAX_OFFSET_DELTA as u32 {
            let r = Relocation {
                offset: MAX_OFFSET_DELTA.into(),
                type_: u8::from(RelocationType::DolphinNop),
                section: 0,
                addend: 0.into(),
            };
            rel.extend_from_slice(r.as_bytes());
            target_delta -= MAX_OFFSET_DELTA as u32;
        }

        let supported_relocation_type = matches!(
            relocation.type_,
            RelocationType::PpcNone
                | RelocationType::PpcAddr32
                | RelocationType::PpcAddr24
                | RelocationType::PpcAddr16
                | RelocationType::PpcAddr16Lo
                | RelocationType::PpcAddr16Hi
                | RelocationType::PpcAddr16Ha
                | RelocationType::PpcAddr14
                | RelocationType::PpcAddr14BrTaken
                | RelocationType::PpcAddr14BrNkTaken
                | RelocationType::PpcRel24
                | RelocationType::DolphinNop
                | RelocationType::DolphinSection
                | RelocationType::DolphinEnd
        );
        if !supported_relocation_type {
            bail!(
                "Unsupported relocation type: {}",
                u8::from(relocation.type_)
            );
        }

        let r = Relocation {
            offset: (target_delta as u16).into(),
            type_: relocation.type_.into(),
            section: relocation.dest_section.0 as u8,
            addend: relocation.addend.into(),
        };
        rel.extend_from_slice(r.as_bytes());
        current_offset = relocation.src_offset;
    }
    let r = Relocation {
        offset: 0.into(),
        type_: RelocationType::DolphinEnd.into(),
        section: 0,
        addend: 0.into(),
    };
    rel.extend_from_slice(r.as_bytes());

    // Write final import infos
    let imports_region =
        &mut rel[import_info_offset..import_info_offset + import_info_buffer.len()];
    imports_region.copy_from_slice(&import_info_buffer);

    Ok(RelocationStats {
        relocations_offset: relocation_offset as u32,
        import_info_offset: import_info_offset as u32,
        import_info_size: import_info_buffer.len() as u32,
    })
}

fn write_module_header(
    elf: &object::File,
    rel: &mut [u8],
    module_id: u32,
    rel_version: RelVersion,
    section_stats: &SectionStats,
    relocation_stats: &RelocationStats,
) -> anyhow::Result<()> {
    let prolog = find_symbol(elf, "_prolog")?;
    let epilog = find_symbol(elf, "_epilog")?;
    let unresolved = find_symbol(elf, "_unresolved")?;

    let header = ModuleHeader {
        id: module_id.into(),
        prev_link: 0.into(),
        next_link: 0.into(),
        section_count: (elf.sections().count() as u32).into(),
        section_info_offset: section_stats.section_info_offset.into(),
        name_offset: 0.into(),
        name_size: 0.into(),
        version: (u8::from(rel_version) as u32).into(),
        total_bss_size: section_stats.total_bss_size.into(),
        relocation_offset: relocation_stats.relocations_offset.into(),
        import_info_offset: relocation_stats.import_info_offset.into(),
        import_info_size: relocation_stats.import_info_size.into(),
        prolog_section: prolog.section_index().unwrap().0 as u8,
        epilog_section: epilog.section_index().unwrap().0 as u8,
        unresolved_section: unresolved.section_index().unwrap().0 as u8,
        pad: 0,
        prolog_offset: (prolog.address() as u32).into(),
        epilog_offset: (epilog.address() as u32).into(),
        unresolved_offset: (unresolved.address() as u32).into(),
    };
    let header_v2 = ModuleV2HeaderAddendum {
        max_align: section_stats.max_align.into(),
        max_bss_align: section_stats.max_bss_align.into(),
    };
    let header_v3 = ModuleV3HeaderAddendum {
        fixed_data_size: relocation_stats.relocations_offset.into(),
    };
    rel[0..header.as_bytes().len()].copy_from_slice(header.as_bytes());
    if rel_version >= RelVersion::V2 {
        let start = header.as_bytes().len();
        let end = start + header_v2.as_bytes().len();
        rel[start..end].copy_from_slice(header_v2.as_bytes());
    }
    if rel_version >= RelVersion::V3 {
        let start = header.as_bytes().len() + header_v2.as_bytes().len();
        let end = start + header_v3.as_bytes().len();
        rel[start..end].copy_from_slice(header_v3.as_bytes());
    }

    Ok(())
}

fn parse_elf(elf_buf: &[u8]) -> anyhow::Result<object::File> {
    let elf = object::read::File::parse(elf_buf)?;
    match elf.architecture() {
        Architecture::PowerPc => {}
        arch => bail!("Unsupported architecture: {arch:?}"),
    };
    ensure!(elf.endianness() == Endianness::Big, "Expected big endian");
    match elf.format() {
        BinaryFormat::Elf => {}
        format => bail!("Unsupported format: {format:?}"),
    }
    Ok(elf)
}

pub fn elf2rel(
    elf_buf: &[u8],
    symbol_map: &[u8],
    module_id: u32,
    rel_version: RelVersion,
) -> anyhow::Result<Vec<u8>> {
    let elf = parse_elf(elf_buf)?;

    let mut rel = Vec::new();

    // Write dummy values for module header until offsets are determined
    rel.extend_from_slice(ModuleHeader::default().as_bytes());
    if rel_version >= RelVersion::V2 {
        rel.extend_from_slice(ModuleV2HeaderAddendum::default().as_bytes());
    }
    if rel_version >= RelVersion::V3 {
        rel.extend_from_slice(ModuleV3HeaderAddendum::default().as_bytes());
    }

    let section_stats = write_sections(&elf, &mut rel)?;
    let relocations =
        extract_relocations(&elf, symbol_map, module_id, &section_stats.section_offsets)?;
    let relocation_stats = write_relocations(
        &mut rel,
        &relocations,
        module_id,
        &section_stats.section_offsets,
    )?;
    write_module_header(
        &elf,
        &mut rel,
        module_id,
        rel_version,
        &section_stats,
        &relocation_stats,
    )?;

    Ok(rel)
}
