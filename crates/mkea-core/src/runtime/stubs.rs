use std::collections::HashMap;

use crate::error::{CoreError, CoreResult};

#[derive(Debug, Clone)]
pub struct StubBinding {
    pub symbol: String,
    pub address: u32,
}

#[derive(Debug, Default)]
pub struct StubRegistry {
    by_addr: HashMap<u32, String>,
    by_symbol: HashMap<String, u32>,
    next_addr: u32,
    end_addr: u32,
    seeded: bool,
}

impl StubRegistry {
    pub fn seed_trampoline(&mut self, start: u32, size: u32) -> CoreResult<()> {
        let end = start
            .checked_add(size)
            .ok_or_else(|| CoreError::Memory("trampoline range overflow".into()))?;
        self.next_addr = start;
        self.end_addr = end;
        self.seeded = true;
        Ok(())
    }

    pub fn insert(&mut self, addr: u32, symbol: impl Into<String>) {
        let symbol = symbol.into();
        self.by_symbol.insert(symbol.clone(), addr);
        self.by_addr.insert(addr, symbol);
    }

    pub fn ensure_symbol(&mut self, symbol: &str) -> CoreResult<u32> {
        if let Some(addr) = self.by_symbol.get(symbol) {
            return Ok(*addr);
        }
        if !self.seeded {
            return Err(CoreError::Memory("trampoline allocator was not seeded".into()));
        }
        let addr = self.next_addr;
        let next = addr
            .checked_add(4)
            .ok_or_else(|| CoreError::Memory("trampoline allocator overflow".into()))?;
        if next > self.end_addr {
            return Err(CoreError::Memory(format!(
                "trampoline page exhausted while binding symbol {symbol}"
            )));
        }
        self.insert(addr, symbol.to_string());
        self.next_addr = next;
        Ok(addr)
    }

    pub fn resolve(&self, addr: u32) -> Option<&str> {
        self.by_addr.get(&addr).map(String::as_str)
    }

    pub fn lookup_symbol(&self, symbol: &str) -> Option<u32> {
        self.by_symbol.get(symbol).copied()
    }

    pub fn bindings(&self) -> Vec<StubBinding> {
        let mut items: Vec<_> = self
            .by_symbol
            .iter()
            .map(|(symbol, address)| StubBinding {
                symbol: symbol.clone(),
                address: *address,
            })
            .collect();
        items.sort_by_key(|item| item.address);
        items
    }

    pub fn len(&self) -> usize {
        self.by_addr.len()
    }
}
