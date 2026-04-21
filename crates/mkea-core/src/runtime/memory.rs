use std::collections::BTreeMap;

use crate::error::{CoreError, CoreResult};

pub const GUEST_PROT_READ: u32 = 1;
pub const GUEST_PROT_WRITE: u32 = 2;
pub const GUEST_PROT_EXEC: u32 = 4;

#[derive(Debug, Clone)]
pub struct MemoryRegion {
    pub name: String,
    pub kind: String,
    pub addr: u32,
    pub size: u32,
    pub prot: u32,
}

impl MemoryRegion {
    pub fn end(&self) -> u32 {
        self.addr.saturating_add(self.size)
    }
}

#[derive(Debug, Clone)]
pub struct GuestWrite {
    pub addr: u32,
    pub size: u32,
    pub kind: String,
}

#[derive(Debug, Default)]
pub struct GuestMemory {
    regions: BTreeMap<u32, MemoryRegion>,
    writes: Vec<GuestWrite>,
}

impl GuestMemory {
    pub fn register_region(&mut self, region: MemoryRegion) -> CoreResult<()> {
        if region.size == 0 {
            return Err(CoreError::Memory(format!("region {} has zero size", region.name)));
        }

        let new_end = region
            .addr
            .checked_add(region.size)
            .ok_or_else(|| CoreError::Memory(format!("region {} overflows guest address space", region.name)))?;

        for existing in self.regions.values() {
            let existing_end = existing
                .addr
                .checked_add(existing.size)
                .ok_or_else(|| CoreError::Memory(format!("existing region {} overflows guest address space", existing.name)))?;

            let overlaps = region.addr < existing_end && new_end > existing.addr;
            if overlaps {
                return Err(CoreError::Memory(format!(
                    "region {} overlaps {} (0x{:08x}-0x{:08x} vs 0x{:08x}-0x{:08x})",
                    region.name,
                    existing.name,
                    region.addr,
                    new_end,
                    existing.addr,
                    existing_end,
                )));
            }
        }

        self.regions.insert(region.addr, region);
        Ok(())
    }

    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    pub fn regions(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.regions.values()
    }

    pub fn writes(&self) -> &[GuestWrite] {
        &self.writes
    }

    pub fn write_bytes(&mut self, addr: u32, size: u32, kind: impl Into<String>) -> CoreResult<()> {
        if size == 0 {
            return Ok(());
        }
        let end = addr
            .checked_add(size)
            .ok_or_else(|| CoreError::Memory("guest write overflows guest address space".into()))?;

        let region = self
            .regions
            .values()
            .find(|region| addr >= region.addr && end <= region.end())
            .ok_or_else(|| {
                CoreError::Memory(format!(
                    "guest write 0x{:08x}-0x{:08x} is not fully covered by a mapped region",
                    addr, end
                ))
            })?;

        if (region.prot & GUEST_PROT_WRITE) == 0 && region.kind != "image" && region.kind != "trampoline" {
            return Err(CoreError::Memory(format!(
                "guest write 0x{:08x}-0x{:08x} targets non-writable region {}",
                addr, end, region.name
            )));
        }

        self.writes.push(GuestWrite {
            addr,
            size,
            kind: kind.into(),
        });
        Ok(())
    }
}

pub fn align_down(x: u32, a: u32) -> u32 {
    x & !(a - 1)
}

pub fn align_up_checked(x: u32, a: u32) -> CoreResult<u32> {
    let add = a
        .checked_sub(1)
        .ok_or_else(|| CoreError::Memory("invalid alignment".into()))?;
    let bumped = x
        .checked_add(add)
        .ok_or_else(|| CoreError::Memory("align_up overflow".into()))?;
    Ok(bumped & !add)
}

pub fn mach_prot_to_guest(prot: i32) -> u32 {
    let mut out = 0u32;
    if (prot & 0x1) != 0 {
        out |= GUEST_PROT_READ;
    }
    if (prot & 0x2) != 0 {
        out |= GUEST_PROT_WRITE;
    }
    if (prot & 0x4) != 0 {
        out |= GUEST_PROT_EXEC;
    }
    if out == 0 {
        GUEST_PROT_READ
    } else {
        out
    }
}

pub fn prot_to_string(prot: u32) -> String {
    let r = if (prot & GUEST_PROT_READ) != 0 { 'R' } else { '-' };
    let w = if (prot & GUEST_PROT_WRITE) != 0 { 'W' } else { '-' };
    let x = if (prot & GUEST_PROT_EXEC) != 0 { 'X' } else { '-' };
    format!("{r}{w}{x}")
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_region_rejects_overlap() {
        let mut mem = GuestMemory::default();
        mem.register_region(MemoryRegion {
            name: "text".into(),
            kind: "image".into(),
            addr: 0x1000,
            size: 0x1000,
            prot: GUEST_PROT_READ | GUEST_PROT_EXEC,
        })
        .unwrap();

        let err = mem
            .register_region(MemoryRegion {
                name: "data".into(),
                kind: "image".into(),
                addr: 0x1800,
                size: 0x1000,
                prot: GUEST_PROT_READ | GUEST_PROT_WRITE,
            })
            .unwrap_err();

        let message = format!("{err}");
        assert!(message.contains("overlaps"));
    }

    #[test]
    fn guest_write_requires_covered_region() {
        let mut mem = GuestMemory::default();
        mem.register_region(MemoryRegion {
            name: "stack".into(),
            kind: "stack".into(),
            addr: 0x7000_0000,
            size: 0x1000,
            prot: GUEST_PROT_READ | GUEST_PROT_WRITE,
        })
        .unwrap();

        mem.write_bytes(0x7000_0010, 4, "stack-init").unwrap();
        assert_eq!(mem.writes().len(), 1);

        let err = mem.write_bytes(0x7000_0ff0, 0x40, "overflow").unwrap_err();
        let message = format!("{err}");
        assert!(message.contains("not fully covered"));
    }

    #[test]
    fn align_up_checked_rounds_to_alignment() {
        assert_eq!(align_down(0x1234, 0x1000), 0x1000);
        assert_eq!(align_up_checked(0x1234, 0x1000).unwrap(), 0x2000);
    }
}
