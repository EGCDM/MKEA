use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const FAT_MAGIC: u32 = 0xCAFEBABE;

const MH_MAGIC: u32 = 0xFEEDFACE;
const MH_CIGAM: u32 = 0xCEFAEDFE;

const CPU_TYPE_ARM: i32 = 12;
const CPU_SUBTYPE_ARM_V6: i32 = 6;
const CPU_SUBTYPE_ARM_V7: i32 = 9;

const LC_SEGMENT: u32 = 0x1;
const LC_SYMTAB: u32 = 0x2;
const LC_UNIXTHREAD: u32 = 0x5;
const LC_DYSYMTAB: u32 = 0xB;
const LC_LOAD_DYLIB: u32 = 0x0C;
const LC_LOAD_WEAK_DYLIB: u32 = 0x18;
const LC_REEXPORT_DYLIB: u32 = 0x1F;
const LC_LAZY_LOAD_DYLIB: u32 = 0x20;
const LC_ENCRYPTION_INFO: u32 = 0x21;
const LC_LOAD_UPWARD_DYLIB: u32 = 0x23;
const LC_MAIN: u32 = 0x8000_0028;

const SECTION_TYPE: u32 = 0x0000_00FF;
const S_NON_LAZY_SYMBOL_POINTERS: u32 = 0x6;
const S_LAZY_SYMBOL_POINTERS: u32 = 0x7;
const S_SYMBOL_STUBS: u32 = 0x8;

const INDIRECT_SYMBOL_LOCAL: u32 = 0x8000_0000;
const INDIRECT_SYMBOL_ABS: u32 = 0x4000_0000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentInfo {
    pub name: String,
    pub vmaddr: u32,
    pub vmsize: u32,
    pub fileoff: u32,
    pub filesize: u32,
    pub maxprot: i32,
    pub initprot: i32,
    pub flags: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionInfo {
    pub sectname: String,
    pub segname: String,
    pub addr: u32,
    pub size: u32,
    pub offset: u32,
    pub flags: u32,
    pub reserved1: u32,
    pub reserved2: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndirectPointerKind {
    Stub,
    Lazy,
    NonLazy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndirectPointer {
    pub address: u32,
    pub symbol: String,
    pub kind: IndirectPointerKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalRelocation {
    pub address: u32,
    pub symbol: String,
    pub pcrel: bool,
    pub length: u8,
    pub rtype: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachProbe {
    pub arch: String,
    pub endianness: String,
    pub ncmds: u32,
    pub sizeofcmds: u32,
    pub has_lc_main: bool,
    pub has_unixthread: bool,
    pub entryoff: Option<u64>,
    pub stacksize: Option<u64>,
    pub entry_pc: Option<u32>,
    pub initial_sp: Option<u32>,
    pub encryption_cryptid: Option<u32>,
    pub dylibs: Vec<String>,
    pub undefined_symbols: Vec<String>,
    pub segments: Vec<SegmentInfo>,
    pub sections: Vec<SectionInfo>,
    pub indirect_pointers: Vec<IndirectPointer>,
    pub external_relocations: Vec<ExternalRelocation>,
}

#[derive(Debug, Clone)]
struct SymtabInfo {
    symoff: u32,
    nsyms: u32,
    stroff: u32,
    strsize: u32,
}

#[derive(Debug, Clone)]
struct DysymtabInfo {
    iundefsym: u32,
    nundefsym: u32,
    indirectsymoff: u32,
    nindirectsyms: u32,
    extreloff: u32,
    nextrel: u32,
}

#[derive(Debug, Clone, Copy)]
enum Endian {
    Little,
    Big,
}

impl Endian {
    fn name(self) -> &'static str {
        match self {
            Endian::Little => "LE",
            Endian::Big => "BE",
        }
    }

    fn read_u32(self, buf: &[u8], off: usize) -> Result<u32> {
        let bytes = take_array::<4>(buf, off)?;
        Ok(match self {
            Endian::Little => u32::from_le_bytes(bytes),
            Endian::Big => u32::from_be_bytes(bytes),
        })
    }

    fn read_i32(self, buf: &[u8], off: usize) -> Result<i32> {
        let bytes = take_array::<4>(buf, off)?;
        Ok(match self {
            Endian::Little => i32::from_le_bytes(bytes),
            Endian::Big => i32::from_be_bytes(bytes),
        })
    }

    fn read_u64(self, buf: &[u8], off: usize) -> Result<u64> {
        let bytes = take_array::<8>(buf, off)?;
        Ok(match self {
            Endian::Little => u64::from_le_bytes(bytes),
            Endian::Big => u64::from_be_bytes(bytes),
        })
    }
}

#[derive(Debug, Clone)]
struct FatSlice<'a> {
    arch: String,
    bytes: &'a [u8],
}

pub fn pick_preferred_slice(buf: &[u8], prefer_arch: &str) -> Result<(String, Vec<u8>)> {
    let slices = split_fat(buf)?;

    if slices.len() == 1 && slices[0].arch == "thin" {
        return Ok(("thin".to_string(), slices[0].bytes.to_vec()));
    }

    if let Some(slice) = slices.iter().find(|s| s.arch == prefer_arch) {
        return Ok((slice.arch.clone(), slice.bytes.to_vec()));
    }

    let first = slices
        .first()
        .ok_or_else(|| anyhow!("fat Mach-O contained no slices"))?;
    Ok((first.arch.clone(), first.bytes.to_vec()))
}

pub fn parse_macho_slice(arch: &str, buf: &[u8]) -> Result<MachProbe> {
    if buf.len() < 28 {
        bail!("Mach-O slice too small: {} bytes", buf.len());
    }

    let endian = detect_macho_endian(buf)?;
    let ncmds = endian.read_u32(buf, 16)?;
    let sizeofcmds = endian.read_u32(buf, 20)?;

    let mut dylibs = Vec::new();
    let mut segments = Vec::new();
    let mut sections = Vec::new();
    let mut undefined_symbols = BTreeSet::new();

    let mut has_lc_main = false;
    let mut has_unixthread = false;
    let mut entryoff = None;
    let mut stacksize = None;
    let mut entry_pc = None;
    let mut initial_sp = None;
    let mut pending_entryoff = None;
    let mut encryption_cryptid = None;

    let mut symtab = None;
    let mut dysymtab = None;
    let mut text_vmaddr = None;

    let mut off = 28usize;
    for _ in 0..ncmds {
        let cmd = endian.read_u32(buf, off)?;
        let cmdsize = endian.read_u32(buf, off + 4)? as usize;
        if cmdsize < 8 {
            bail!("invalid load command size {} at offset 0x{:x}", cmdsize, off);
        }
        if off + cmdsize > buf.len() {
            bail!(
                "load command exceeds Mach-O slice: off=0x{:x} cmdsize=0x{:x} len=0x{:x}",
                off,
                cmdsize,
                buf.len()
            );
        }

        match cmd {
            LC_SEGMENT => {
                let segname = read_fixed_cstr(&buf[off + 8..off + 24]);
                let vmaddr = endian.read_u32(buf, off + 24)?;
                let vmsize = endian.read_u32(buf, off + 28)?;
                let fileoff = endian.read_u32(buf, off + 32)?;
                let filesize = endian.read_u32(buf, off + 36)?;
                let maxprot = endian.read_i32(buf, off + 40)?;
                let initprot = endian.read_i32(buf, off + 44)?;
                let nsects = endian.read_u32(buf, off + 48)?;
                let flags = endian.read_u32(buf, off + 52)?;

                if segname == "__TEXT" {
                    text_vmaddr = Some(vmaddr);
                    if entry_pc.is_none() {
                        if let Some(pending) = pending_entryoff {
                            let pending32 = u32::try_from(pending)
                                .context("LC_MAIN entryoff does not fit into u32")?;
                            entry_pc = Some(vmaddr.wrapping_add(pending32));
                            pending_entryoff = None;
                        }
                    }
                }

                segments.push(SegmentInfo {
                    name: segname.clone(),
                    vmaddr,
                    vmsize,
                    fileoff,
                    filesize,
                    maxprot,
                    initprot,
                    flags,
                });

                let section_base = off + 56;
                let section_size = 68usize;
                for index in 0..nsects {
                    let sec_off = section_base + (index as usize) * section_size;
                    if sec_off + section_size > off + cmdsize {
                        bail!(
                            "section {} for segment {} exceeds load command bounds",
                            index,
                            segname
                        );
                    }
                    let sectname = read_fixed_cstr(&buf[sec_off..sec_off + 16]);
                    let sec_segname = read_fixed_cstr(&buf[sec_off + 16..sec_off + 32]);
                    let addr = endian.read_u32(buf, sec_off + 32)?;
                    let size = endian.read_u32(buf, sec_off + 36)?;
                    let offset = endian.read_u32(buf, sec_off + 40)?;
                    let flags = endian.read_u32(buf, sec_off + 56)?;
                    let reserved1 = endian.read_u32(buf, sec_off + 60)?;
                    let reserved2 = endian.read_u32(buf, sec_off + 64)?;
                    sections.push(SectionInfo {
                        sectname,
                        segname: sec_segname,
                        addr,
                        size,
                        offset,
                        flags,
                        reserved1,
                        reserved2,
                    });
                }
            }
            LC_LOAD_DYLIB | LC_LOAD_WEAK_DYLIB | LC_REEXPORT_DYLIB | LC_LOAD_UPWARD_DYLIB | LC_LAZY_LOAD_DYLIB => {
                let name_rel = endian.read_u32(buf, off + 8)? as usize;
                if name_rel < cmdsize {
                    let dylib_name = read_cstr(buf, off + name_rel);
                    if !dylib_name.is_empty() {
                        dylibs.push(dylib_name);
                    }
                }
            }
            LC_MAIN => {
                has_lc_main = true;
                let raw_entryoff = endian.read_u64(buf, off + 8)?;
                let raw_stacksize = endian.read_u64(buf, off + 16)?;
                entryoff = Some(raw_entryoff);
                stacksize = Some(raw_stacksize);
                if let Some(text) = text_vmaddr {
                    let entryoff32 = u32::try_from(raw_entryoff)
                        .context("LC_MAIN entryoff does not fit into u32")?;
                    entry_pc = Some(text.wrapping_add(entryoff32));
                } else {
                    pending_entryoff = Some(raw_entryoff);
                }
            }
            LC_UNIXTHREAD => {
                has_unixthread = true;
                let mut p = off + 8;
                let end = off + cmdsize;
                while p + 8 <= end {
                    let flavor = endian.read_u32(buf, p)?;
                    let count = endian.read_u32(buf, p + 4)?;
                    p += 8;
                    let nbytes = (count as usize)
                        .checked_mul(4)
                        .ok_or_else(|| anyhow!("thread state size overflow"))?;
                    if p + nbytes > end {
                        break;
                    }

                    if flavor == 1 && count >= 16 {
                        initial_sp = Some(endian.read_u32(buf, p + 13 * 4)?);
                        entry_pc = Some(endian.read_u32(buf, p + 15 * 4)?);
                        break;
                    }

                    p += nbytes;
                }
            }
            LC_ENCRYPTION_INFO => {
                encryption_cryptid = Some(endian.read_u32(buf, off + 16)?);
            }
            LC_SYMTAB => {
                symtab = Some(SymtabInfo {
                    symoff: endian.read_u32(buf, off + 8)?,
                    nsyms: endian.read_u32(buf, off + 12)?,
                    stroff: endian.read_u32(buf, off + 16)?,
                    strsize: endian.read_u32(buf, off + 20)?,
                });
            }
            LC_DYSYMTAB => {
                dysymtab = Some(DysymtabInfo {
                    iundefsym: endian.read_u32(buf, off + 8 + 4 * 4)?,
                    nundefsym: endian.read_u32(buf, off + 8 + 5 * 4)?,
                    indirectsymoff: endian.read_u32(buf, off + 8 + 12 * 4)?,
                    nindirectsyms: endian.read_u32(buf, off + 8 + 13 * 4)?,
                    extreloff: endian.read_u32(buf, off + 8 + 14 * 4)?,
                    nextrel: endian.read_u32(buf, off + 8 + 15 * 4)?,
                });
            }
            _ => {}
        }

        off += cmdsize;
    }

    if entry_pc.is_none() {
        if let (Some(text), Some(pending)) = (text_vmaddr, pending_entryoff) {
            let pending32 = u32::try_from(pending)
                .context("LC_MAIN entryoff does not fit into u32")?;
            entry_pc = Some(text.wrapping_add(pending32));
        }
    }

    let mut indirect_pointers = Vec::new();
    let mut external_relocations = Vec::new();

    if let Some(symtab) = &symtab {
        let symbol_names = build_symbol_names(buf, endian, symtab)?;

        if let Some(dysymtab) = &dysymtab {
            collect_undefined_symbols(&symbol_names, dysymtab, &mut undefined_symbols)?;
            indirect_pointers = parse_indirect_pointers(buf, endian, &sections, &symbol_names, dysymtab)?;
            external_relocations = parse_external_relocations(buf, endian, &symbol_names, dysymtab)?;
        }
    }

    Ok(MachProbe {
        arch: arch.to_string(),
        endianness: endian.name().to_string(),
        ncmds,
        sizeofcmds,
        has_lc_main,
        has_unixthread,
        entryoff,
        stacksize,
        entry_pc,
        initial_sp,
        encryption_cryptid,
        dylibs,
        undefined_symbols: undefined_symbols.into_iter().collect(),
        segments,
        sections,
        indirect_pointers,
        external_relocations,
    })
}

fn split_fat(buf: &[u8]) -> Result<Vec<FatSlice<'_>>> {
    if buf.len() < 8 {
        bail!("buffer too small for Mach-O/FAT detection: {} bytes", buf.len());
    }

    let magic = u32::from_be_bytes(take_array::<4>(buf, 0)?);
    if magic != FAT_MAGIC {
        return Ok(vec![FatSlice {
            arch: "thin".to_string(),
            bytes: buf,
        }]);
    }

    let nfat_arch = u32::from_be_bytes(take_array::<4>(buf, 4)?) as usize;
    let mut off = 8usize;
    let mut out = Vec::with_capacity(nfat_arch);

    for _ in 0..nfat_arch {
        let cputype = i32::from_be_bytes(take_array::<4>(buf, off)?);
        let cpusubtype = i32::from_be_bytes(take_array::<4>(buf, off + 4)?);
        let slice_off = u32::from_be_bytes(take_array::<4>(buf, off + 8)?) as usize;
        let slice_size = u32::from_be_bytes(take_array::<4>(buf, off + 12)?) as usize;
        off += 20;

        let end = slice_off
            .checked_add(slice_size)
            .ok_or_else(|| anyhow!("fat slice overflow"))?;
        if end > buf.len() {
            bail!(
                "fat slice extends beyond file: off=0x{:x} size=0x{:x} len=0x{:x}",
                slice_off,
                slice_size,
                buf.len()
            );
        }

        out.push(FatSlice {
            arch: cpu_name(cputype, cpusubtype),
            bytes: &buf[slice_off..end],
        });
    }

    Ok(out)
}

fn detect_macho_endian(buf: &[u8]) -> Result<Endian> {
    let le = u32::from_le_bytes(take_array::<4>(buf, 0)?);
    if le == MH_MAGIC {
        return Ok(Endian::Little);
    }
    if le == MH_CIGAM {
        return Ok(Endian::Big);
    }

    let be = u32::from_be_bytes(take_array::<4>(buf, 0)?);
    if be == MH_MAGIC {
        return Ok(Endian::Big);
    }
    if be == MH_CIGAM {
        return Ok(Endian::Little);
    }

    bail!("not a 32-bit Mach-O slice (magic le={:#x} be={:#x})", le, be)
}

fn build_symbol_names(buf: &[u8], endian: Endian, symtab: &SymtabInfo) -> Result<Vec<String>> {
    let str_start = symtab.stroff as usize;
    let str_end = str_start
        .checked_add(symtab.strsize as usize)
        .ok_or_else(|| anyhow!("string table overflow"))?;
    if str_end > buf.len() {
        bail!(
            "string table out of bounds: off=0x{:x} size=0x{:x} len=0x{:x}",
            str_start,
            symtab.strsize,
            buf.len()
        );
    }
    let strtab = &buf[str_start..str_end];

    let mut out = Vec::with_capacity(symtab.nsyms as usize);
    for i in 0..symtab.nsyms {
        let sym_off = symtab.symoff as usize + (i as usize) * 12;
        if sym_off + 12 > buf.len() {
            bail!("nlist entry {} exceeds Mach-O length", i);
        }
        let n_strx = endian.read_u32(buf, sym_off)? as usize;
        let name = read_cstr_from_table(strtab, n_strx);
        out.push(name);
    }
    Ok(out)
}

fn collect_undefined_symbols(
    symbol_names: &[String],
    dysymtab: &DysymtabInfo,
    out: &mut BTreeSet<String>,
) -> Result<()> {
    let start = dysymtab.iundefsym as usize;
    let count = dysymtab.nundefsym as usize;
    let end = start
        .checked_add(count)
        .ok_or_else(|| anyhow!("undefined symbol range overflow"))?;
    if end > symbol_names.len() {
        bail!(
            "undefined symbol range exceeds symbol table: start={} count={} total={}",
            start,
            count,
            symbol_names.len()
        );
    }

    for name in &symbol_names[start..end] {
        let trimmed = name.trim_start_matches('_');
        if !trimmed.is_empty() {
            out.insert(trimmed.to_string());
        }
    }
    Ok(())
}

fn parse_indirect_pointers(
    buf: &[u8],
    endian: Endian,
    sections: &[SectionInfo],
    symbol_names: &[String],
    dysymtab: &DysymtabInfo,
) -> Result<Vec<IndirectPointer>> {
    if dysymtab.nindirectsyms == 0 {
        return Ok(Vec::new());
    }

    let indirect_off = dysymtab.indirectsymoff as usize;
    let indirect_size = (dysymtab.nindirectsyms as usize)
        .checked_mul(4)
        .ok_or_else(|| anyhow!("indirect symbol table size overflow"))?;
    if indirect_off + indirect_size > buf.len() {
        bail!("indirect symbol table exceeds Mach-O slice");
    }

    let mut out = Vec::new();

    for section in sections {
        let kind = match section.flags & SECTION_TYPE {
            S_SYMBOL_STUBS => Some(IndirectPointerKind::Stub),
            S_LAZY_SYMBOL_POINTERS => Some(IndirectPointerKind::Lazy),
            S_NON_LAZY_SYMBOL_POINTERS => Some(IndirectPointerKind::NonLazy),
            _ => None,
        };
        let Some(kind) = kind else {
            continue;
        };

        let count = match kind {
            IndirectPointerKind::Stub => {
                let stub_size = section.reserved2.max(1);
                section.size / stub_size
            }
            IndirectPointerKind::Lazy | IndirectPointerKind::NonLazy => section.size / 4,
        };
        if count == 0 {
            continue;
        }

        let base_index = section.reserved1 as usize;
        for slot in 0..count as usize {
            let indirect_index = base_index + slot;
            if indirect_index >= dysymtab.nindirectsyms as usize {
                break;
            }
            let raw = endian.read_u32(buf, indirect_off + indirect_index * 4)?;
            if (raw & INDIRECT_SYMBOL_LOCAL) != 0 || (raw & INDIRECT_SYMBOL_ABS) != 0 {
                continue;
            }
            let sym_index = raw as usize;
            let Some(name) = symbol_names.get(sym_index) else {
                continue;
            };
            let trimmed = name.trim_start_matches('_');
            if trimmed.is_empty() {
                continue;
            }
            let address = match kind {
                IndirectPointerKind::Stub => {
                    section.addr.saturating_add((slot as u32).saturating_mul(section.reserved2.max(1)))
                }
                IndirectPointerKind::Lazy | IndirectPointerKind::NonLazy => {
                    section.addr.saturating_add((slot as u32) * 4)
                }
            };
            out.push(IndirectPointer {
                address,
                symbol: trimmed.to_string(),
                kind,
            });
        }
    }

    Ok(out)
}

fn parse_external_relocations(
    buf: &[u8],
    endian: Endian,
    symbol_names: &[String],
    dysymtab: &DysymtabInfo,
) -> Result<Vec<ExternalRelocation>> {
    if dysymtab.nextrel == 0 {
        return Ok(Vec::new());
    }

    let extrel_off = dysymtab.extreloff as usize;
    let extrel_size = (dysymtab.nextrel as usize)
        .checked_mul(8)
        .ok_or_else(|| anyhow!("external relocation table size overflow"))?;
    if extrel_off + extrel_size > buf.len() {
        bail!("external relocation table exceeds Mach-O slice");
    }

    let mut out = Vec::new();
    for index in 0..dysymtab.nextrel as usize {
        let off = extrel_off + index * 8;
        let r_address = endian.read_i32(buf, off)?;
        let r_info = endian.read_u32(buf, off + 4)?;

        let symnum = (r_info & 0x00FF_FFFF) as usize;
        let pcrel = ((r_info >> 24) & 0x1) != 0;
        let length = ((r_info >> 25) & 0x3) as u8;
        let is_extern = ((r_info >> 27) & 0x1) != 0;
        let rtype = ((r_info >> 28) & 0xF) as u8;

        if !is_extern || length != 2 {
            continue;
        }
        let Some(name) = symbol_names.get(symnum) else {
            continue;
        };
        let trimmed = name.trim_start_matches('_');
        if trimmed.is_empty() {
            continue;
        }

        out.push(ExternalRelocation {
            address: r_address as u32,
            symbol: trimmed.to_string(),
            pcrel,
            length,
            rtype,
        });
    }

    Ok(out)
}

fn cpu_name(cputype: i32, cpusubtype: i32) -> String {
    if cputype == CPU_TYPE_ARM {
        if cpusubtype == CPU_SUBTYPE_ARM_V6 {
            return "armv6".to_string();
        }
        if cpusubtype == CPU_SUBTYPE_ARM_V7 {
            return "armv7".to_string();
        }
        return format!("arm(sub={cpusubtype})");
    }
    format!("cpu({cputype},sub={cpusubtype})")
}

fn read_fixed_cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn read_cstr(buf: &[u8], off: usize) -> String {
    if off >= buf.len() {
        return String::new();
    }
    let end = buf[off..]
        .iter()
        .position(|b| *b == 0)
        .map(|pos| off + pos)
        .unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[off..end]).into_owned()
}

fn read_cstr_from_table(strtab: &[u8], off: usize) -> String {
    if off >= strtab.len() {
        return String::new();
    }
    let end = strtab[off..]
        .iter()
        .position(|b| *b == 0)
        .map(|pos| off + pos)
        .unwrap_or(strtab.len());
    String::from_utf8_lossy(&strtab[off..end]).into_owned()
}

fn take_array<const N: usize>(buf: &[u8], off: usize) -> Result<[u8; N]> {
    let end = off
        .checked_add(N)
        .ok_or_else(|| anyhow!("slice overflow"))?;
    let slice = buf
        .get(off..end)
        .ok_or_else(|| anyhow!("out-of-bounds read off=0x{:x} size=0x{:x}", off, N))?;
    let mut out = [0u8; N];
    out.copy_from_slice(slice);
    Ok(out)
}
