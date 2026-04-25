#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatusFlags(u8);

impl StatusFlags {
    pub const CARRY: Self = Self(0b0000_0001);
    pub const ZERO: Self = Self(0b0000_0010);
    pub const INTERRUPT_DISABLE: Self = Self(0b0000_0100);
    pub const DECIMAL: Self = Self(0b0000_1000);
    pub const BREAK: Self = Self(0b0001_0000);
    pub const UNUSED: Self = Self(0b0010_0000);
    pub const OVERFLOW: Self = Self(0b0100_0000);
    pub const NEGATIVE: Self = Self(0b1000_0000);

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

pub struct MemoryBus {
    memory: [u8; 0x10000],
}

impl MemoryBus {
    pub fn new() -> Self {
        Self {
            memory: [0; 0x10000],
        }
    }

    pub fn read(&self, addr: u16) -> u8 {
        self.memory[addr as usize]
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        self.memory[addr as usize] = value;
    }

    pub fn load(&mut self, start: u16, bytes: &[u8]) {
        let start = start as usize;
        self.memory[start..start + bytes.len()].copy_from_slice(bytes);
    }
}

pub struct Cpu6502 {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub pc: u16,
    pub sp: u8,
    pub status: StatusFlags,
}

impl Cpu6502 {
    pub fn new() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            pc: 0,
            sp: 0xfd,
            status: StatusFlags::UNUSED,
        }
    }

    pub fn reset(&mut self, _bus: &mut MemoryBus) {
        unimplemented!("load the reset vector and initialize CPU state")
    }

    pub fn step(&mut self, _bus: &mut MemoryBus) -> u8 {
        unimplemented!("execute one 6502 instruction")
    }

    pub fn run_until_brk(
        &mut self,
        _bus: &mut MemoryBus,
        _instruction_limit: usize,
    ) -> Result<usize, String> {
        unimplemented!("run until BRK or the instruction limit")
    }
}
