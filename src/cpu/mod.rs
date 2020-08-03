mod cartridge;
mod instructions;
mod address_space;
mod registers;
mod addressing_mode;

use super::utils::Clocked;
use registers::Registers;
use address_space::{
    AddressSpace,
    ZeroPageAddressSpace,
    StackAddressSpace,
    RamAddressSpace,
    PpuRegistersAddressSpace,
    ApuRegistersAddressSpace,
    IORegistersAddressSpace,
    NullAddressSpace,
};
use addressing_mode::{
    AddressingMode,
    Implicit,
    Accumulator,
    Immediate,
    Relative,
    MemoryAccess,
};
use cartridge::{
    Mapper,
    DummyMapper,
};

pub use cartridge::load_cartridge;
use crate::cpu::address_space::CartridgeAddressSpace;

enum Interrupts
{
    Break,
    Reset,
    IRQ,
    NMI,
}

pub enum InstructionResult
{
    Ok,
    NOP,
    Branch(u32), // number of cycle needed to take the branch (2 if page boundary crossed else 1)
    OAMDMA,
}

pub struct Cpu
{
    registers: Registers,
    pub cycles: u64,
    wait_cycles: u32,
    // internal ram : size 0x0800
    zero_page_ram: [u8; 0x0100],
    stack: [u8; 0x0100],
    internal_ram: [u8; 0x0600],
    // cartridge space
    cartridge: Box<dyn Mapper>,
}

impl Cpu
{
    pub fn new_dummy() -> Cpu
    {
        Cpu {
            registers: Registers::new(),
            cycles: 7,
            wait_cycles: 0,
            zero_page_ram: [0; 0x0100],
            stack: [0; 0x0100],
            internal_ram: [0; 0x0600],
            cartridge: Box::new(DummyMapper::new()),
        }
    }

    pub fn new(cartridge: Box<dyn Mapper>) -> Cpu
    {
        let mut cpu = Cpu {
            registers: Registers::new(),
            cycles: 7,
            wait_cycles: 0,
            zero_page_ram: [0; 0x0100],
            stack: [0; 0x0100],
            internal_ram: [0; 0x0600],
            cartridge,
        };
        cpu.registers.pc = cpu.load(0xFFFE) as u16 | (cpu.load(0xFFFF) as u16) << 8;
        cpu
    }

    fn corresponding_address_space(&self, address: u16) -> Box<dyn AddressSpace>
    {
        let first_nibble = (address >> 8) as u8;
        let second_nibble = address as u8;

        match (first_nibble, second_nibble) {
            (x, index) if x <= 0x1F => match (x % 0x08, index) {
                (0x00, index) => Box::new(ZeroPageAddressSpace::new(index)),
                (0x01, index) => Box::new(StackAddressSpace::new(index)),
                (0x02..=0x07, _) => Box::new(RamAddressSpace::new(address % 0x0800 - 0x0200)),
                (_, _) => Box::new(NullAddressSpace::new()), // should never happen
            },
            (x, y) if x <= 0x40 => Box::new(PpuRegistersAddressSpace::new((y % 0x08) as u16)),
            (0x40, x) if x <= 0x13 => Box::new(ApuRegistersAddressSpace::new(x as u16)),
            (0x40, x) if x <= 0x17 => Box::new(IORegistersAddressSpace::new(x as u16)),
            (0x40, x) if x <= 0x1F => Box::new(NullAddressSpace::new()), // unused APU and IO functionnalities
            _ => Box::new(CartridgeAddressSpace::new(address))
        }
    }

    pub fn push(&mut self, data: u8)
    {
        self.write(0x0100 | self.registers.stack_pointer as u16, data);
        self.registers.stack_pointer = self.registers.stack_pointer.wrapping_sub(1);
    }

    pub fn top(&self) -> u8
    {
        self.load(0x0100 | self.registers.stack_pointer.wrapping_add(1) as u16)
    }

    pub fn pop(&mut self) -> u8
    {
        let data = self.top();
        self.registers.stack_pointer = self.registers.stack_pointer.wrapping_add(1);
        data
    }

    pub fn load(&self, address: u16) -> u8
    {
        let address_space = self.corresponding_address_space(address);

        address_space.read(&self)
    }

    pub fn write(&mut self, address: u16, data: u8)
    {
        let address_space = self.corresponding_address_space(address);

        address_space.write(self, data);
    }

    fn load_byte_at_pc(&self) -> u8 { self.load(self.registers.pc) }

    fn increment_pc(&mut self) { self.registers.pc += 1 }

    pub fn fetch(&mut self) -> u8
    {
        let data = self.load_byte_at_pc();
        self.increment_pc();
        data
    }

    pub fn interrupt(&mut self, kind: Interrupts)
    {
        let (vector, b_flag, side_effect_flags) = match kind {
            Interrupts::Break => (0xFFFEu16, 0b0011_0000u8, 0b0000_0100u8),
            Interrupts::Reset => (0xFFFCu16, 0b0000_0000u8, 0b0000_0000u8),
            Interrupts::IRQ => (0xFFFEu16, 0b0010_0000u8, 0b0000_0100u8),
            Interrupts::NMI => (0xFFFAu16, 0b0010_0000u8, 0b0000_0100u8),
        };

        let address = self.registers.pc;
        self.push((address >> 8) as u8);
        self.push(address as u8);
        self.push(self.registers.p.get_byte() | b_flag);

        self.registers.pc = self.load(vector) as u16 | (self.load(vector + 1) as u16) << 8;
        self.registers.p.set_byte(side_effect_flags);
    }

    pub fn set_pc(&mut self, address: u16) { self.registers.pc = address }

    fn get_addressing_mode(&mut self, opcode: u8) -> Box<dyn AddressingMode>
    {
        match opcode {
            //+00
            0x20 => Box::new(MemoryAccess::new_absolute(self)),
            0x80 | 0xA0 | 0xC0 | 0xE0 => Box::new(Immediate::new(self)),
            //+01
            x if x & 0x1F == 0x01 => Box::new(MemoryAccess::new_indexed_indirect(self, self.registers.x)),
            //+02
            0x82 | 0xA2 | 0xC2 | 0xE2 => Box::new(Immediate::new(self)),
            //+03
            x if x & 0x1F == 0x03 => Box::new(MemoryAccess::new_indexed_zero_page(self, self.registers.x)),
            //+04
            x if x & 0x1F == 0x04 => Box::new(MemoryAccess::new_zero_page(self)),
            //+05
            x if x & 0x1F == 0x05 => Box::new(MemoryAccess::new_zero_page(self)),
            //+06
            x if x & 0x1F == 0x06 => Box::new(MemoryAccess::new_zero_page(self)),
            //+07
            x if x & 0x1F == 0x07 => Box::new(MemoryAccess::new_zero_page(self)),
            //+08
            //+09
            x if x & 0x1F == 0x09 => Box::new(Immediate::new(self)),
            //+0A
            0x0A | 0x2A | 0x4A | 0x6A => Box::new(Accumulator{}),
            //+0B
            //+0C
            0x6C => Box::new(MemoryAccess::new_indirect(self)),
            x if x & 0x1F == 0x0C => Box::new(MemoryAccess::new_absolute(self)),
            //+0D
            x if x & 0x1F == 0x0D => Box::new(MemoryAccess::new_absolute(self)),
            //+0E
            x if x & 0x1F == 0x0E => Box::new(MemoryAccess::new_absolute(self)),
            //+0F
            x if x & 0x1F == 0x0F => Box::new(MemoryAccess::new_absolute(self)),
            //+10
            x if x & 0x1F == 0x10 => Box::new(Relative::new(self)),
            //+11
            x if x & 0x1F == 0x11 => Box::new(MemoryAccess::new_indirect_indexed(self, self.registers.y)),
            //+12
            //+13
            x if x & 0x1F == 0x13 => Box::new(MemoryAccess::new_indirect_indexed(self, self.registers.y)),
            //+14
            x if x & 0x1F == 0x14 => Box::new(MemoryAccess::new_indexed_zero_page(self, self.registers.x)),
            //+15
            x if x & 0x1F == 0x15 => Box::new(MemoryAccess::new_indexed_zero_page(self, self.registers.x)),
            //+16
            0x96 | 0xB6 => Box::new(MemoryAccess::new_indexed_zero_page(self, self.registers.y)),
            x if x & 0x1F == 0x16 => Box::new(MemoryAccess::new_indexed_zero_page(self, self.registers.x)),
            //+17
            0x97 | 0xB7 => Box::new(MemoryAccess::new_indexed_zero_page(self, self.registers.y)),
            x if x & 0x1F == 0x17 => Box::new(MemoryAccess::new_indexed_zero_page(self, self.registers.x)),
            //+18
            //+19
            x if x & 0x1F == 0x19 => Box::new(MemoryAccess::new_indexed_absolute(self, self.registers.y)),
            //+1A
            //+1B
            x if x & 0x1F == 0x1B => Box::new(MemoryAccess::new_indexed_absolute(self, self.registers.y)),
            //+1C
            x if x & 0x1F == 0x1C => Box::new(MemoryAccess::new_indexed_absolute(self, self.registers.x)),
            //+1D
            x if x & 0x1F == 0x1D => Box::new(MemoryAccess::new_indexed_absolute(self, self.registers.x)),
            //+1E
            0x9E | 0xBE => Box::new(MemoryAccess::new_indexed_absolute(self, self.registers.y)),
            x if x & 0x1F == 0x1E => Box::new(MemoryAccess::new_indexed_absolute(self, self.registers.x)),
            //+1F
            0x9F | 0xBF => Box::new(MemoryAccess::new_indexed_absolute(self, self.registers.y)),
            x if x & 0x1F == 0x1F => Box::new(MemoryAccess::new_indexed_absolute(self, self.registers.x)),
            _ => Box::new(Implicit{})
        }
    }

    fn get_wait_cycles(&mut self, opcode: u8, page_boundary_crossed: bool) -> u32
    {
        match opcode {
            // stack operation
            0x00 => 7,
            0x40 | 0x60 | 0x20 => 6,
            0x08 | 0x48 => 3,
            0x28 | 0x68 => 4,
            // absolute
            //// Read
            0x4C => 3,
            0xAD | 0xAE | 0xAC | 0x4D | 0x2D | 0x0D | 0x6D | 0xED | 0xCD | 0x2C | 0x0C | 0xCC | 0xEC => 4,
            //// RMW
            0x0E | 0x4E | 0x2E | 0x6E | 0xEE | 0xCE => 6,
            //// write
            0x8D | 0x8E | 0x8C => 4,
            // zero page
            //// Read
            0xA5 | 0xA6 | 0xA4 | 0x45 | 0x25 | 0x05 | 0x65 | 0xE5 | 0xC5 | 0x24 | 0x04 | 0x44 | 0x64 | 0xC4 | 0xE4 => 3,
            //// RMW
            0x06 | 0x46 | 0x26 | 0x66 | 0xE6 | 0xC6 => 5,
            //// write
            0x85 | 0x86 | 0x84 => 3,
            // indexed zero page
            //// Read
            0xB5 | 0xB6 | 0xB4 | 0x55 | 0x35 | 0x15 | 0x75 | 0xF5 | 0xD5 | 0x34 | 0x14 | 0x54 | 0x74 | 0xD4 | 0xF4 => 4,
            //// RMW
            0x16 | 0x56 | 0x36 | 0x76 | 0xF6 | 0xD6 => 6,
            //// write
            0x95 | 0x96 | 0x94 => 4,
            // indexed absolute
            //// Read
            0xBC | 0x19 | 0x1D | 0x39 | 0x3D | 0x59 | 0x5D | 0x79 | 0x7D | 0xB9 | 0xBD | 0xD9 | 0xDD | 0xF9 | 0xFD | 0xBE | 0x1C | 0x3C | 0x5C | 0x7C | 0xDC | 0xFC => if page_boundary_crossed {5} else {4},
            //// RMW
            0x1E | 0x3E | 0x5E | 0x7E | 0xDE | 0xFE => 7,
            //// write
            0x99 | 0x9D => 5,
            // Relative
            0x10 | 0x30 | 0x50 | 0x70 | 0x90 | 0xB0 | 0xD0 | 0xF0 => 2, // todo, add cycle if branch is taken and if page boundary is crossed in branched instruction
            // indexed indirect
            //// Read
            0x01 | 0x21 | 0x41 | 0x61 | 0xA1 | 0xC1 | 0xE1 => 6,
            //// write
            0x81 => 6,
            // indirect indexed
            //// Read
            0x11 | 0x31 | 0x51 | 0x71 | 0xB1 | 0xD1 | 0xF1 => if page_boundary_crossed {6} else {5},
            //// write
            0x91 => 6,
            // indirect
            0x6C => 5,
            // implicit, accumulator, immediate
            _ => 2
        }
    }

    fn get_instruction_name(&self, opcode: u8) -> &str
    {
        match opcode {
            // Control operations
            //// Stack operation
            0x00 => "BRK",
            0x40 => "RTI",
            0x60 => "RTS",
            0x08 => "PHP",
            0x28 => "PLP",
            0x48 => "PHA",
            0x68 => "PLA",
            0x20 => "JSR",
            //// Jump operation
            0x4C | 0x6C => "JMP",
            //// Branch operation
            0x10 => "BPL",
            0x30 => "BMI",
            0x50 => "BVC",
            0x70 => "BVS",
            0x90 => "BCC",
            0xB0 => "BCS",
            0xD0 => "BNE",
            0xF0 => "BEQ",
            ////
            0x88 => "DEY",
            0xA8 => "TAY",
            0xC8 => "INY",
            0xE8 => "INX",
            0x18 => "CLC",
            0x38 => "SEC",
            0x58 => "CLI",
            0x78 => "SEI",
            0x98 => "TYA",
            0xB8 => "CLV",
            0xD8 => "CLD",
            0xF8 => "SED",
            0x80 | 0x04 | 0x44 | 0x64 | 0x0C | 0x14 | 0x34 | 0x54 | 0x74 | 0xD4 | 0xF4 | 0x1C | 0x3C | 0x5C | 0x7C | 0xDC | 0xFC => "NOP",
            0x9C => "NOP", // undocumented instructions
            x if x & 0xE0 == 0x20 && x & 0x03 == 0x00 => "BIT",
            x if x & 0xE0 == 0x80 && x & 0x03 == 0x00 => "STY",
            x if x & 0xE0 == 0xA0 && x & 0x03 == 0x00 => "LDY",
            x if x & 0xE0 == 0xC0 && x & 0x03 == 0x00 => "CPY",
            x if x & 0xE0 == 0xE0 && x & 0x03 == 0x00 => "CPx",
            // ALU operations
            0x89 => "NOP",
            x if x & 0xE0 == 0x00 && x & 0x03 == 0x01 => "ORA",
            x if x & 0xE0 == 0x20 && x & 0x03 == 0x01 => "AND",
            x if x & 0xE0 == 0x40 && x & 0x03 == 0x01 => "EOR",
            x if x & 0xE0 == 0x60 && x & 0x03 == 0x01 => "ADC",
            x if x & 0xE0 == 0x80 && x & 0x03 == 0x01 => "STA",
            x if x & 0xE0 == 0xA0 && x & 0x03 == 0x01 => "LDA",
            x if x & 0xE0 == 0xC0 && x & 0x03 == 0x01 => "CMP",
            x if x & 0xE0 == 0xE0 && x & 0x03 == 0x01 => "SBC",
            // RMW operations
            0x82 | 0xC2 | 0xE2 | 0xEA | 0x1A | 0x3A | 0x5A | 0x7A | 0xDA | 0xFA => "NOP",
            0x02 | 0x22 | 0x42 | 0x62 | 0x12 | 0x31 | 0x52 | 0x72 | 0x92 | 0xB2 | 0xD2 | 0xF2 | 0x9E  => "NOP", // undocumented instructions
            x if x & 0xE0 == 0x00 && x & 0x03 == 0x02 => "ASL",
            x if x & 0xE0 == 0x20 && x & 0x03 == 0x02 => "ROL",
            x if x & 0xE0 == 0x40 && x & 0x03 == 0x02 => "LSR",
            x if x & 0xE0 == 0x60 && x & 0x03 == 0x02 => "ROR",
            x if x & 0xE0 == 0x80 && x & 0x03 == 0x02 => "STX",
            x if x & 0xE0 == 0xA0 && x & 0x03 == 0x02 => "LDX",
            x if x & 0xE0 == 0xC0 && x & 0x03 == 0x02 => "DEC",
            x if x & 0xE0 == 0xE0 && x & 0x03 == 0x02 => "INC",
            _ => "NOP", // undocumented instructions
        }
    }


    // returns the number of cycle to wait
    fn execute_instruction(&mut self, opcode: u8) -> u32
    {
        let addressing_mode = self.get_addressing_mode(opcode);
        let wait_cycles = self.get_wait_cycles(opcode, addressing_mode.page_boundary_crossed());
        let instruction_result = match opcode {
            // Control operations
            0x00 => self.brk(&*addressing_mode),
            0x40 => self.rti(&*addressing_mode),
            0x60 => self.rts(&*addressing_mode),
            //// Stack operation
            0x08 => self.php(&*addressing_mode),
            0x28 => self.plp(&*addressing_mode),
            0x48 => self.pha(&*addressing_mode),
            0x68 => self.pla(&*addressing_mode),
            //// Jump operation
            0x4C | 0x6C => self.jmp(&*addressing_mode),
            0x20 => self.jsr(&*addressing_mode),
            //// Branch operation
            0x10 => self.bpl(&*addressing_mode),
            0x30 => self.bmi(&*addressing_mode),
            0x50 => self.bvc(&*addressing_mode),
            0x70 => self.bvs(&*addressing_mode),
            0x90 => self.bcc(&*addressing_mode),
            0xB0 => self.bcs(&*addressing_mode),
            0xD0 => self.bne(&*addressing_mode),
            0xF0 => self.beq(&*addressing_mode),
            ////
            0x18 => self.clc(&*addressing_mode),
            0x38 => self.sec(&*addressing_mode),
            0x58 => self.cli(&*addressing_mode),
            0x78 => self.sei(&*addressing_mode),
            0x88 => self.dey(&*addressing_mode),
            0x98 => self.tya(&*addressing_mode),
            0xA8 => self.tay(&*addressing_mode),
            0xB8 => self.clv(&*addressing_mode),
            0xC8 => self.iny(&*addressing_mode),
            0xCA => self.dex(&*addressing_mode),
            0xD8 => self.cld(&*addressing_mode),
            0xE8 => self.inx(&*addressing_mode),
            0xF8 => self.sed(&*addressing_mode),
            0x80 | 0x04 | 0x44 | 0x64 | 0x0C | 0x14 | 0x34 | 0x54 | 0x74 | 0xD4 | 0xF4 | 0x1C | 0x3C | 0x5C | 0x7C | 0xDC | 0xFC => InstructionResult::NOP,
            0x9C => InstructionResult::NOP, // undocumented instructions
            x if x & 0xE0 == 0x20 && x & 0x03 == 0x00 => self.bit(&*addressing_mode),
            x if x & 0xE0 == 0x80 && x & 0x03 == 0x00 => self.sty(&*addressing_mode),
            x if x & 0xE0 == 0xA0 && x & 0x03 == 0x00 => self.ldy(&*addressing_mode),
            x if x & 0xE0 == 0xC0 && x & 0x03 == 0x00 => self.cpy(&*addressing_mode),
            x if x & 0xE0 == 0xE0 && x & 0x03 == 0x00 => self.cpx(&*addressing_mode),
            // ALU operations
            0x89 => InstructionResult::NOP,
            x if x & 0xE0 == 0x00 && x & 0x03 == 0x01 => self.ora(&*addressing_mode),
            x if x & 0xE0 == 0x20 && x & 0x03 == 0x01 => self.and(&*addressing_mode),
            x if x & 0xE0 == 0x40 && x & 0x03 == 0x01 => self.eor(&*addressing_mode),
            x if x & 0xE0 == 0x60 && x & 0x03 == 0x01 => self.adc(&*addressing_mode),
            x if x & 0xE0 == 0x80 && x & 0x03 == 0x01 => self.sta(&*addressing_mode),
            x if x & 0xE0 == 0xA0 && x & 0x03 == 0x01 => self.lda(&*addressing_mode),
            x if x & 0xE0 == 0xC0 && x & 0x03 == 0x01 => self.cmp(&*addressing_mode),
            x if x & 0xE0 == 0xE0 && x & 0x03 == 0x01 => self.sbc(&*addressing_mode),
            // RMW operations
            0x8A => self.txa(&*addressing_mode),
            0xAA => self.tax(&*addressing_mode),
            0x9A => self.txs(&*addressing_mode),
            0xBA => self.tsx(&*addressing_mode),
            0x82 | 0xC2 | 0xE2 | 0xEA | 0x1A | 0x3A | 0x5A | 0x7A | 0xDA | 0xFA => InstructionResult::NOP,
            0x02 | 0x22 | 0x42 | 0x62 | 0x12 | 0x31 | 0x52 | 0x72 | 0x92 | 0xB2 | 0xD2 | 0xF2 | 0x9E  => InstructionResult::NOP, // undocumented instructions
            x if x & 0xE0 == 0x00 && x & 0x03 == 0x02 => self.asl(&*addressing_mode),
            x if x & 0xE0 == 0x20 && x & 0x03 == 0x02 => self.rol(&*addressing_mode),
            x if x & 0xE0 == 0x40 && x & 0x03 == 0x02 => self.lsr(&*addressing_mode),
            x if x & 0xE0 == 0x60 && x & 0x03 == 0x02 => self.ror(&*addressing_mode),
            x if x & 0xE0 == 0x80 && x & 0x03 == 0x02 => self.stx(&*addressing_mode),
            x if x & 0xE0 == 0xA0 && x & 0x03 == 0x02 => self.ldx(&*addressing_mode),
            x if x & 0xE0 == 0xC0 && x & 0x03 == 0x02 => self.dec(&*addressing_mode),
            x if x & 0xE0 == 0xE0 && x & 0x03 == 0x02 => self.inc(&*addressing_mode),
            _ => InstructionResult::NOP, // undocumented instructions
        };
        wait_cycles + match instruction_result {
            InstructionResult::Ok | InstructionResult::NOP => 0,
            InstructionResult::Branch(cycles) => cycles,
            InstructionResult::OAMDMA => if self.cycles % 2 == 1 {514} else {513},
        }
    }

    fn trace(&self)
    {
        println!(
            "{:04X}  {:02X} {:02X} {:02X}  {:3}                             A:{:02X} X:{:02X} Y:{:02X} P:{:02X} SP:{:02X}             CYC:{}",
            self.registers.pc,
            self.load(self.registers.pc), self.load(self.registers.pc + 1), self.load(self.registers.pc + 2),
            self.get_instruction_name(self.load(self.registers.pc)),
            self.registers.a,
            self.registers.x,
            self.registers.y,
            self.registers.p.get_byte() | 0b0010_0000,
            self.registers.stack_pointer,
            self.cycles,
        );
    }
}

impl Clocked for Cpu
{
    fn clock(&mut self)
    {
        match self.wait_cycles {
            0 => {
                self.trace();
                let opcode = self.fetch();
                self.wait_cycles = self.execute_instruction(opcode);
            },
            _ => self.wait_cycles -= 1
        }
        self.cycles += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod address_space
    {
        use super::*;

        mod zero_page
        {
            use super::*;

            #[test]
            fn test_read()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.zero_page_ram[0x04] = 4;

                assert_eq!(cpu.load(0x0004), 4);
            }

            #[test]
            fn test_write()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.zero_page_ram[0x04] = 0;

                cpu.write(0x04, 4);
                assert_eq!(cpu.zero_page_ram[0x04], 4);
            }

            #[test]
            fn test_read_mirrored()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.zero_page_ram[0x04] = 4;

                assert_eq!(cpu.load(0x0804), 4);
                assert_eq!(cpu.load(0x1004), 4);
                assert_eq!(cpu.load(0x1804), 4);
            }

            #[test]
            fn test_write_mirrored()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.zero_page_ram[0x04] = 0;

                cpu.write(0x0804, 8);
                assert_eq!(cpu.zero_page_ram[0x04], 8);
                cpu.write(0x1004, 10);
                assert_eq!(cpu.zero_page_ram[0x04], 10);
                cpu.write(0x1804, 18);
                assert_eq!(cpu.zero_page_ram[0x04], 18);
            }
        }

        mod stack
        {
            use super::*;

            #[test]
            fn test_read()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.stack[0x04] = 4;

                assert_eq!(cpu.load(0x0104), 4);
            }

            #[test]
            fn test_write()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.stack[0x04] = 0;

                cpu.write(0x0104, 4);
                assert_eq!(cpu.stack[0x04], 4);
            }

            #[test]
            fn test_read_mirrored()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.stack[0x04] = 4;

                assert_eq!(cpu.load(0x0904), 4);
                assert_eq!(cpu.load(0x1104), 4);
                assert_eq!(cpu.load(0x1904), 4);
            }

            #[test]
            fn test_write_mirrored()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.stack[0x04] = 0;

                cpu.write(0x0904, 9);
                assert_eq!(cpu.stack[0x04], 9);
                cpu.write(0x1104, 11);
                assert_eq!(cpu.stack[0x04], 11);
                cpu.write(0x1904, 19);
                assert_eq!(cpu.stack[0x04], 19);
            }
        }

        mod internal_ram
        {
            use super::*;

            #[test]
            fn test_read()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.internal_ram[0x0004] = 4;

                assert_eq!(cpu.load(0x0204), 4);
            }

            #[test]
            fn test_write()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.internal_ram[0x0004] = 0;

                cpu.write(0x0204, 4);
                assert_eq!(cpu.internal_ram[0x0004], 4);
            }

            #[test]
            fn test_read_mirrored()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.internal_ram[0x0004] = 4;

                assert_eq!(cpu.load(0x0A04), 4);
                assert_eq!(cpu.load(0x1204), 4);
                assert_eq!(cpu.load(0x1A04), 4);
            }

            #[test]
            fn test_write_mirrored()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.internal_ram[0x0004] = 0;

                cpu.write(0x0A04, 0x0A);
                assert_eq!(cpu.internal_ram[0x0004], 0x0A);
                cpu.write(0x1204, 0x12);
                assert_eq!(cpu.internal_ram[0x0004], 0x12);
                cpu.write(0x1A04, 0x1A);
                assert_eq!(cpu.internal_ram[0x0004], 0x1A);
            }
        }
    }

    mod addressing_mode
    {
        use super::*;

        mod accumulator
        {
            use super::*;

            #[test]
            fn test_read()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.a = 4;
                let addressing_mode = Accumulator{};

                assert_eq!(addressing_mode.read(&cpu), 4);
            }

            #[test]
            fn test_write()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.a = 0;
                let addressing_mode = Accumulator{};

                addressing_mode.write(&mut cpu, 4);
                assert_eq!(cpu.registers.a, 4);
            }
        }

        mod immediate
        {
            use super::*;

            #[test]
            fn test_read()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 4;
                let addressing_mode = Immediate::new(&mut cpu);

                assert_eq!(addressing_mode.read(&cpu), 4);
            }
        }

        mod relative
        {
            use super::*;

            #[test]
            fn test_read()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 4;
                let addressing_mode = Relative::new(&mut cpu);

                assert_eq!(addressing_mode.read(&cpu), 4);
            }
        }

        mod memory_access
        {
            use super::*;

            mod zero_page
            {
                use super::*;

                #[test]
                fn test_read()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.zero_page_ram[0x04] = 8;
                    cpu.internal_ram[0x00] = 4;
                    let addressing_mode = MemoryAccess::new_zero_page(&mut cpu);

                    assert_eq!(addressing_mode.read(&cpu), 8);
                }

                #[test]
                fn test_indexed_read()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.zero_page_ram[0x08] = 8;
                    cpu.internal_ram[0x00] = 4;
                    let addressing_mode = MemoryAccess::new_indexed_zero_page(&mut cpu, 4);

                    assert_eq!(addressing_mode.read(&cpu), 8);
                }

                #[test]
                fn test_indexed_with_crossed_boundaries_read()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.zero_page_ram[0x03] = 8;
                    cpu.internal_ram[0x00] = 0xFF;
                    let addressing_mode = MemoryAccess::new_indexed_zero_page(&mut cpu, 4);

                    assert_eq!(addressing_mode.read(&cpu), 8);
                }

                #[test]
                fn test_write()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.zero_page_ram[0x04] = 0;
                    cpu.internal_ram[0x00] = 4;
                    let addressing_mode = MemoryAccess::new_zero_page(&mut cpu);

                    addressing_mode.write(&mut cpu, 8);
                    assert_eq!(cpu.zero_page_ram[0x04], 8);
                }

                #[test]
                fn test_indexed_write()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.zero_page_ram[0x08] = 0;
                    cpu.internal_ram[0x00] = 4;
                    let addressing_mode = MemoryAccess::new_indexed_zero_page(&mut cpu, 4);

                    addressing_mode.write(&mut cpu, 8);
                    assert_eq!(cpu.zero_page_ram[0x08], 8);
                }

                #[test]
                fn test_indexed_with_crossed_boundaries_write()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.zero_page_ram[0x03] = 0;
                    cpu.internal_ram[0x00] = 0xFF;
                    let addressing_mode = MemoryAccess::new_indexed_zero_page(&mut cpu, 4);

                    addressing_mode.write(&mut cpu, 8);
                    assert_eq!(cpu.zero_page_ram[0x03], 8);
                }
            }

            mod absolute
            {
                use super::*;

                #[test]
                fn test_read()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0004] = 4;
                    cpu.internal_ram[0x00] = 0x04;
                    cpu.internal_ram[0x01] = 0x02;
                    let addressing_mode = MemoryAccess::new_absolute(&mut cpu);

                    assert_eq!(addressing_mode.read(&cpu), 4);
                }

                #[test]
                fn test_indexed_read()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0008] = 4;
                    cpu.internal_ram[0x00] = 0x04;
                    cpu.internal_ram[0x01] = 0x02;
                    let addressing_mode = MemoryAccess::new_indexed_absolute(&mut cpu, 4);

                    assert_eq!(addressing_mode.read(&cpu), 4);
                }

                #[test]
                fn test_write()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0004] = 0;
                    cpu.internal_ram[0x00] = 0x04;
                    cpu.internal_ram[0x01] = 0x02;
                    let addressing_mode = MemoryAccess::new_absolute(&mut cpu);

                    addressing_mode.write(&mut cpu, 8);
                    assert_eq!(cpu.internal_ram[0x0004], 8);
                }

                #[test]
                fn test_indexed_write()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0008] = 0;
                    cpu.internal_ram[0x00] = 0x04;
                    cpu.internal_ram[0x01] = 0x02;
                    let addressing_mode = MemoryAccess::new_indexed_absolute(&mut cpu, 4);

                    addressing_mode.write(&mut cpu, 8);
                    assert_eq!(cpu.internal_ram[0x0008], 8);
                }
            }

            mod indirect
            {
                use super::*;

                #[test]
                fn test_read()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0070] = 7;
                    cpu.internal_ram[0x0004] = 0x70;
                    cpu.internal_ram[0x0005] = 0x02;
                    cpu.internal_ram[0x00] = 0x04;
                    cpu.internal_ram[0x01] = 0x02;
                    let addressing_mode = MemoryAccess::new_indirect(&mut cpu);

                    assert_eq!(addressing_mode.read(&cpu), 7);
                }

                #[test]
                fn test_indexed_indirect_read()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0070] = 7;
                    cpu.zero_page_ram[0x08] = 0x70;
                    cpu.zero_page_ram[0x09] = 0x02;
                    cpu.internal_ram[0x00] = 0x04;
                    let addressing_mode = MemoryAccess::new_indexed_indirect(&mut cpu, 4);

                    assert_eq!(addressing_mode.read(&cpu), 7);
                }

                #[test]
                fn test_indexed_indirect_with_crossed_boundaries_read()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0070] = 7;
                    cpu.zero_page_ram[0x03] = 0x70;
                    cpu.zero_page_ram[0x04] = 0x02;
                    cpu.internal_ram[0x00] = 0xFF;
                    let addressing_mode = MemoryAccess::new_indexed_indirect(&mut cpu, 4);

                    assert_eq!(addressing_mode.read(&cpu), 7);

                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0070] = 7;
                    cpu.zero_page_ram[0xFF] = 0x70;
                    cpu.zero_page_ram[0x00] = 0x02;
                    cpu.internal_ram[0x00] = 0xFE;
                    let addressing_mode = MemoryAccess::new_indexed_indirect(&mut cpu, 1);

                    assert_eq!(addressing_mode.read(&cpu), 7);

                }

                #[test]
                fn test_indirect_indexed_read()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0074] = 7;
                    cpu.zero_page_ram[0x04] = 0x70;
                    cpu.zero_page_ram[0x05] = 0x02;
                    cpu.internal_ram[0x00] = 0x04;
                    let addressing_mode = MemoryAccess::new_indirect_indexed(&mut cpu, 4);

                    assert_eq!(addressing_mode.read(&cpu), 7);
                }

                #[test]
                fn test_indirect_indexed_with_crossed_boundaries_read()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0074] = 7;
                    cpu.zero_page_ram[0xFF] = 0x70;
                    cpu.zero_page_ram[0x00] = 0x02;
                    cpu.internal_ram[0x00] = 0xFF;
                    let addressing_mode = MemoryAccess::new_indirect_indexed(&mut cpu, 4);

                    assert_eq!(addressing_mode.read(&cpu), 7);
                }

                #[test]
                fn test_write()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0070] = 0;
                    cpu.internal_ram[0x0004] = 0x70;
                    cpu.internal_ram[0x0005] = 0x02;
                    cpu.internal_ram[0x00] = 0x04;
                    cpu.internal_ram[0x01] = 0x02;
                    let addressing_mode = MemoryAccess::new_indirect(&mut cpu);


                    addressing_mode.write(&mut cpu, 8);
                    assert_eq!(cpu.internal_ram[0x0070], 8);
                }

                #[test]
                fn test_indexed_indirect_write()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0070] = 0;
                    cpu.zero_page_ram[0x08] = 0x70;
                    cpu.zero_page_ram[0x09] = 0x02;
                    cpu.internal_ram[0x00] = 0x04;
                    let addressing_mode = MemoryAccess::new_indexed_indirect(&mut cpu, 4);

                    addressing_mode.write(&mut cpu, 8);
                    assert_eq!(cpu.internal_ram[0x0070], 8);
                }

                #[test]
                fn test_indexed_indirect_with_crossed_boundaries_write()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0070] = 7;
                    cpu.zero_page_ram[0x03] = 0x70;
                    cpu.zero_page_ram[0x04] = 0x02;
                    cpu.internal_ram[0x00] = 0xFF;
                    let addressing_mode = MemoryAccess::new_indexed_indirect(&mut cpu, 4);

                    addressing_mode.write(&mut cpu, 8);
                    assert_eq!(cpu.internal_ram[0x0070], 8);

                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0070] = 7;
                    cpu.zero_page_ram[0xFF] = 0x70;
                    cpu.zero_page_ram[0x00] = 0x02;
                    cpu.internal_ram[0x00] = 0xFE;
                    let addressing_mode = MemoryAccess::new_indexed_indirect(&mut cpu, 1);

                    addressing_mode.write(&mut cpu, 8);
                    assert_eq!(cpu.internal_ram[0x0070], 8);
                }

                #[test]
                fn test_indirect_indexed_write()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0074] = 0;
                    cpu.zero_page_ram[0x04] = 0x70;
                    cpu.zero_page_ram[0x05] = 0x02;
                    cpu.internal_ram[0x00] = 0x04;
                    let addressing_mode = MemoryAccess::new_indirect_indexed(&mut cpu, 4);

                    addressing_mode.write(&mut cpu, 8);
                    assert_eq!(cpu.internal_ram[0x0074], 8);
                }


                #[test]
                fn test_indirect_indexed_with_crossed_boundaries_write()
                {
                    let mut cpu = Cpu::new_dummy();
                    cpu.registers.pc = 0x0200;
                    cpu.internal_ram[0x0074] = 0;
                    cpu.zero_page_ram[0xFF] = 0x70;
                    cpu.zero_page_ram[0x00] = 0x02;
                    cpu.internal_ram[0x00] = 0xFF;
                    let addressing_mode = MemoryAccess::new_indirect_indexed(&mut cpu, 4);

                    addressing_mode.write(&mut cpu, 8);
                    assert_eq!(cpu.internal_ram[0x0074], 8);
                }
            }
        }
    }

    mod instructions
    {
        use super::*;

        mod lda
        {
            use super::*;

            #[test]
            fn test_immediate()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0xA9);

                assert_eq!(cpu.registers.a, 0x04);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x06;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0xA5);

                assert_eq!(cpu.registers.a, 0x06);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0xFF;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x00;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xB5);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x00] = 0x04;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0xB5);

                assert_eq!(cpu.registers.a, 0x04);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x1F; // same as 0x07 due to mirroring
                cpu.internal_ram[0x0504] = 0x06;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0xAD);

                assert_eq!(cpu.registers.a, 0x06);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x06;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0xBD);

                assert_eq!(cpu.registers.a, 0x06);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x06;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x00;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xB9);

                assert_eq!(cpu.registers.a, 0x06);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x06;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0xBD);

                assert_eq!(cpu.registers.a, 0x06);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x06;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0xB9);

                assert_eq!(cpu.registers.a, 0x06);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_indirect()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x07] = 0x04;
                cpu.zero_page_ram[0x08] = 0x06;
                cpu.internal_ram[0x0404] = 0x07;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0xA1);

                assert_eq!(cpu.registers.a, 0x07);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_indirect_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x07] = 0x04;
                cpu.zero_page_ram[0x08] = 0x06;
                cpu.internal_ram[0x0404] = 0x07;
                cpu.registers.x = 0x08;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0xA1);

                assert_eq!(cpu.registers.a, 0x07);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indirect_indexed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x06;
                cpu.internal_ram[0x0407] = 0x07;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0xB1);

                assert_eq!(cpu.registers.a, 0x07);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indirect_indexed_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x06;
                cpu.internal_ram[0x0502] = 0x07;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0xB1);

                assert_eq!(cpu.registers.a, 0x07);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x00;
                cpu.registers.a = 0xFF;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xA9);
                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0xFF;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xA9);
                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0x00;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xA9);
                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x80;
                cpu.registers.a = 0x04;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xA9);
                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod ldx
        {
            use super::*;

            #[test]
            fn test_immediate()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.x = 0x00;

                let wait_cycles = cpu.execute_instruction(0xA2);

                assert_eq!(cpu.registers.x, 0x04);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x06;
                cpu.registers.x = 0x00;

                let wait_cycles = cpu.execute_instruction(0xA6);

                assert_eq!(cpu.registers.x, 0x06);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0xFF;
                cpu.registers.y = 0x01;
                cpu.registers.x = 0x00;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xB6);

                assert_eq!(cpu.registers.x, 0xFF);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x00] = 0x04;
                cpu.registers.y = 0x01;
                cpu.registers.x = 0x00;

                let wait_cycles = cpu.execute_instruction(0xB6);

                assert_eq!(cpu.registers.x, 0x04);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x1F; // same as 0x07 due to mirroring
                cpu.internal_ram[0x0504] = 0x06;
                cpu.registers.x = 0x00;

                let wait_cycles = cpu.execute_instruction(0xAE);

                assert_eq!(cpu.registers.x, 0x06);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x06;
                cpu.registers.y = 0x01;
                cpu.registers.x = 0x00;

                let wait_cycles = cpu.execute_instruction(0xBE);

                assert_eq!(cpu.registers.x, 0x06);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x06;
                cpu.registers.y = 0x03;
                cpu.registers.x = 0x00;

                let wait_cycles = cpu.execute_instruction(0xBE);

                assert_eq!(cpu.registers.x, 0x06);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x00;
                cpu.registers.x = 0xFF;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xA2);
                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.x = 0xFF;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xA2);
                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.x = 0x00;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xA2);
                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x80;
                cpu.registers.x = 0x04;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xA2);
                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod ldy
        {
            use super::*;

            #[test]
            fn test_immediate()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.y = 0x00;

                let wait_cycles = cpu.execute_instruction(0xA0);

                assert_eq!(cpu.registers.y, 0x04);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x06;
                cpu.registers.y = 0x00;

                let wait_cycles = cpu.execute_instruction(0xA4);

                assert_eq!(cpu.registers.y, 0x06);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0xFF;
                cpu.registers.x = 0x01;
                cpu.registers.y = 0x00;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xB4);

                assert_eq!(cpu.registers.y, 0xFF);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x00] = 0x04;
                cpu.registers.x = 0x01;
                cpu.registers.y = 0x00;

                let wait_cycles = cpu.execute_instruction(0xB4);

                assert_eq!(cpu.registers.y, 0x04);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x1F; // same as 0x07 due to mirroring
                cpu.internal_ram[0x0504] = 0x06;
                cpu.registers.y = 0x00;

                let wait_cycles = cpu.execute_instruction(0xAC);

                assert_eq!(cpu.registers.y, 0x06);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x06;
                cpu.registers.x = 0x01;
                cpu.registers.y = 0x00;

                let wait_cycles = cpu.execute_instruction(0xBC);

                assert_eq!(cpu.registers.y, 0x06);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x06;
                cpu.registers.x = 0x03;
                cpu.registers.y = 0x00;

                let wait_cycles = cpu.execute_instruction(0xBC);

                assert_eq!(cpu.registers.y, 0x06);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x00;
                cpu.registers.y = 0xFF;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xA0);
                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.y = 0xFF;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xA0);
                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.y = 0x00;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xA9);
                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x80;
                cpu.registers.y = 0x04;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xA9);
                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod sta
        {
            use super::*;

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x01;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0x85);

                assert_eq!(cpu.zero_page_ram[0x04], 0x06);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x01;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xFF;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x95);

                assert_eq!(cpu.zero_page_ram[0x05], 0xFF);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x00] = 0x01;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0x95);

                assert_eq!(cpu.zero_page_ram[0x00], 0x04);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x1F; // same as 0x07 due to mirroring
                cpu.internal_ram[0x0504] = 0x00;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0x8D);

                assert_eq!(cpu.internal_ram[0x0504], 0x06);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x01;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0x9D);

                assert_eq!(cpu.internal_ram[0x0205], 0x06);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x01;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x99);

                assert_eq!(cpu.internal_ram[0x0205], 0x06);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_absolute_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x01;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0x9D);

                assert_eq!(cpu.internal_ram[0x0302], 0x06);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x06;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0x99);

                assert_eq!(cpu.internal_ram[0x0302], 0x06);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_indirect()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x07] = 0x04;
                cpu.zero_page_ram[0x08] = 0x06;
                cpu.internal_ram[0x0404] = 0x01;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x07;

                let wait_cycles = cpu.execute_instruction(0x81);

                assert_eq!(cpu.internal_ram[0x0404], 0x07);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_indirect_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x07] = 0x04;
                cpu.zero_page_ram[0x08] = 0x06;
                cpu.internal_ram[0x0404] = 0x01;
                cpu.registers.x = 0x08;
                cpu.registers.a = 0x07;

                let wait_cycles = cpu.execute_instruction(0x81);

                assert_eq!(cpu.internal_ram[0x0404], 0x07);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indirect_indexed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x06;
                cpu.internal_ram[0x0407] = 0x01;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x07;

                let wait_cycles = cpu.execute_instruction(0x91);

                assert_eq!(cpu.internal_ram[0x0407], 0x07);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indirect_indexed_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x06;
                cpu.internal_ram[0x0502] = 0x01;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x07;

                let wait_cycles = cpu.execute_instruction(0x91);

                assert_eq!(cpu.internal_ram[0x0502], 0x07);
                assert_eq!(wait_cycles, 6);
            }
        }

        mod stx
        {
            use super::*;

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x01;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0x86);

                assert_eq!(cpu.zero_page_ram[0x04], 0x06);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x01;
                cpu.registers.y = 0x01;
                cpu.registers.x = 0xFF;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x96);

                assert_eq!(cpu.zero_page_ram[0x05], 0xFF);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x00] = 0x01;
                cpu.registers.y = 0x01;
                cpu.registers.x = 0x04;

                let wait_cycles = cpu.execute_instruction(0x96);

                assert_eq!(cpu.zero_page_ram[0x00], 0x04);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x1F; // same as 0x07 due to mirroring
                cpu.internal_ram[0x0504] = 0x01;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0x8E);

                assert_eq!(cpu.internal_ram[0x0504], 0x06);
                assert_eq!(wait_cycles, 4);
            }
        }

        mod sty
        {
            use super::*;

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x01;
                cpu.registers.y = 0x06;

                let wait_cycles = cpu.execute_instruction(0x84);

                assert_eq!(cpu.zero_page_ram[0x04], 0x06);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x01;
                cpu.registers.x = 0x01;
                cpu.registers.y = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x94);

                assert_eq!(cpu.zero_page_ram[0x05], 0xFF);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x00] = 0x01;
                cpu.registers.x = 0x01;
                cpu.registers.y = 0x04;

                let wait_cycles = cpu.execute_instruction(0x94);

                assert_eq!(cpu.zero_page_ram[0x00], 0x04);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x1F; // same as 0x07 due to mirroring
                cpu.internal_ram[0x0504] = 0x00;
                cpu.registers.y = 0x06;

                let wait_cycles = cpu.execute_instruction(0x8C);

                assert_eq!(cpu.internal_ram[0x0504], 0x06);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x01;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0x9D);

                assert_eq!(cpu.internal_ram[0x0205], 0x06);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x01;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x99);

                assert_eq!(cpu.internal_ram[0x0205], 0x06);
                assert_eq!(wait_cycles, 5);
            }
        }

        mod tax
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xAA);

                assert_eq!(cpu.registers.x, 0x04);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0xFF;
                cpu.registers.a = 0x00;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xAA);
                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x04;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xAA);
                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x00;
                cpu.registers.a = 0x04;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xAA);
                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x80;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xAA);
                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod tay
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xA8);

                assert_eq!(cpu.registers.y, 0x04);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0xFF;
                cpu.registers.a = 0x00;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xA8);
                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x04;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xA8);
                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x00;
                cpu.registers.a = 0x04;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xA8);
                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0x80;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xA8);
                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod txa
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0x8A);

                assert_eq!(cpu.registers.a, 0x03);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0xFF;
                cpu.registers.x = 0x00;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0x8A);
                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x01;
                cpu.registers.x = 0x04;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0x8A);
                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x00;
                cpu.registers.x = 0x04;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0x8A);
                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.x = 0x80;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0x8A);
                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod tya
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x03;
                cpu.registers.y = 0x04;

                let wait_cycles = cpu.execute_instruction(0x98);

                assert_eq!(cpu.registers.y, 0x04);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0xFF;
                cpu.registers.y = 0x00;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0x98);
                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x01;
                cpu.registers.y = 0x04;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0x98);
                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x00;
                cpu.registers.y = 0x04;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0x98);
                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.y = 0x80;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0x98);
                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod tsx
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x03;
                cpu.registers.stack_pointer = 0x04;

                let wait_cycles = cpu.execute_instruction(0xBA);

                assert_eq!(cpu.registers.x, 0x04);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0xFF;
                cpu.registers.stack_pointer = 0x00;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xBA);
                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x01;
                cpu.registers.stack_pointer = 0x04;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xBA);
                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x00;
                cpu.registers.stack_pointer = 0x04;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xBA);
                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x04;
                cpu.registers.stack_pointer = 0x80;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xBA);
                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod txs
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x03;
                cpu.registers.stack_pointer = 0x04;

                let wait_cycles = cpu.execute_instruction(0x9A);

                assert_eq!(cpu.registers.stack_pointer, 0x03);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0xFF;
                cpu.registers.x = 0x00;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0x9A);
                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0x01;
                cpu.registers.x = 0x04;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0x9A);
                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0xFF;
                cpu.registers.x = 0x00;
                cpu.registers.p.zero = true;

                cpu.execute_instruction(0x9A);
                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0x01;
                cpu.registers.x = 0x04;
                cpu.registers.p.zero = true;

                cpu.execute_instruction(0x9A);
                assert_eq!(cpu.registers.p.zero, true);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0x00;
                cpu.registers.x = 0x04;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0x9A);
                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0x04;
                cpu.registers.x = 0x80;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0x9A);
                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0x00;
                cpu.registers.x = 0x04;
                cpu.registers.p.negative = true;

                cpu.execute_instruction(0x9A);
                assert_eq!(cpu.registers.p.negative, true);

                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0x04;
                cpu.registers.x = 0x80;
                cpu.registers.p.negative = true;

                cpu.execute_instruction(0x9A);
                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod pha
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.stack_pointer = 0xFD;
                cpu.stack[0xFD] = 0x00;

                let wait_cycles = cpu.execute_instruction(0x48);

                assert_eq!(cpu.stack[0xFD], 0x04);
                assert_eq!(cpu.registers.stack_pointer, 0xFC);
                assert_eq!(wait_cycles, 3);
            }
        }

        mod php
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = false;
                cpu.registers.p.zero = false;
                cpu.registers.p.interrupt_disable = false;
                cpu.registers.p.decimal = false;
                cpu.registers.p.overflow = false;
                cpu.registers.p.negative = false;
                cpu.registers.stack_pointer = 0xFD;
                cpu.stack[0xFD] = 0x00;

                let wait_cycles = cpu.execute_instruction(0x08);

                assert_eq!(cpu.stack[0xFD], 0b0011_0000);
                assert_eq!(cpu.registers.stack_pointer, 0xFC);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = true;
                cpu.registers.p.zero = true;
                cpu.registers.p.interrupt_disable = true;
                cpu.registers.p.decimal = true;
                cpu.registers.p.overflow = true;
                cpu.registers.p.negative = true;
                cpu.registers.stack_pointer = 0xFD;
                cpu.stack[0xFD] = 0x00;

                let wait_cycles = cpu.execute_instruction(0x08);

                assert_eq!(cpu.stack[0xFD], 0b1111_1111);
                assert_eq!(cpu.registers.stack_pointer, 0xFC);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_carry_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = true;
                cpu.registers.p.zero = false;
                cpu.registers.p.interrupt_disable = false;
                cpu.registers.p.decimal = false;
                cpu.registers.p.overflow = false;
                cpu.registers.p.negative = false;
                cpu.registers.stack_pointer = 0xFD;
                cpu.stack[0xFD] = 0x00;

                let wait_cycles = cpu.execute_instruction(0x08);

                assert_eq!(cpu.stack[0xFD], 0b0011_0001);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = false;
                cpu.registers.p.zero = true;
                cpu.registers.p.interrupt_disable = false;
                cpu.registers.p.decimal = false;
                cpu.registers.p.overflow = false;
                cpu.registers.p.negative = false;
                cpu.registers.stack_pointer = 0xFD;
                cpu.stack[0xFD] = 0x00;

                let wait_cycles = cpu.execute_instruction(0x08);

                assert_eq!(cpu.stack[0xFD], 0b0011_0010);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_interrupt_disable_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = false;
                cpu.registers.p.zero = false;
                cpu.registers.p.interrupt_disable = true;
                cpu.registers.p.decimal = false;
                cpu.registers.p.overflow = false;
                cpu.registers.p.negative = false;
                cpu.registers.stack_pointer = 0xFD;
                cpu.stack[0xFD] = 0x00;

                let wait_cycles = cpu.execute_instruction(0x08);

                assert_eq!(cpu.stack[0xFD], 0b0011_0100);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_decimal_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = false;
                cpu.registers.p.zero = false;
                cpu.registers.p.interrupt_disable = false;
                cpu.registers.p.decimal = true;
                cpu.registers.p.overflow = false;
                cpu.registers.p.negative = false;
                cpu.registers.stack_pointer = 0xFD;
                cpu.stack[0xFD] = 0x00;

                let wait_cycles = cpu.execute_instruction(0x08);

                assert_eq!(cpu.stack[0xFD], 0b0011_1000);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_overflow_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = false;
                cpu.registers.p.zero = false;
                cpu.registers.p.interrupt_disable = false;
                cpu.registers.p.decimal = false;
                cpu.registers.p.overflow = true;
                cpu.registers.p.negative = false;
                cpu.registers.stack_pointer = 0xFD;
                cpu.stack[0xFD] = 0x00;

                let wait_cycles = cpu.execute_instruction(0x08);

                assert_eq!(cpu.stack[0xFD], 0b0111_0000);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = false;
                cpu.registers.p.zero = false;
                cpu.registers.p.interrupt_disable = false;
                cpu.registers.p.decimal = false;
                cpu.registers.p.overflow = false;
                cpu.registers.p.negative = true;
                cpu.registers.stack_pointer = 0xFD;
                cpu.stack[0xFD] = 0x00;

                let wait_cycles = cpu.execute_instruction(0x08);

                assert_eq!(cpu.stack[0xFD], 0b1011_0000);
                assert_eq!(wait_cycles, 3);
            }
        }

        mod pla
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x00;
                cpu.registers.stack_pointer = 0xFC;
                cpu.stack[0xFD] = 0x04;

                let wait_cycles = cpu.execute_instruction(0x68);

                assert_eq!(cpu.registers.a, 0x04);
                assert_eq!(cpu.registers.stack_pointer, 0xFD);
                assert_eq!(wait_cycles, 4);
            }
        }

        mod plp
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = false;
                cpu.registers.p.zero = false;
                cpu.registers.p.interrupt_disable = false;
                cpu.registers.p.decimal = false;
                cpu.registers.p.overflow = false;
                cpu.registers.p.negative = false;
                cpu.registers.stack_pointer = 0xFC;
                cpu.stack[0xFD] = 0b1111_1111;

                let wait_cycles = cpu.execute_instruction(0x28);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(cpu.registers.p.interrupt_disable, true);
                assert_eq!(cpu.registers.p.decimal, true);
                assert_eq!(cpu.registers.p.overflow, true);
                assert_eq!(cpu.registers.p.negative, true);
                assert_eq!(cpu.registers.stack_pointer, 0xFD);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = true;
                cpu.registers.p.zero = true;
                cpu.registers.p.interrupt_disable = true;
                cpu.registers.p.decimal = true;
                cpu.registers.p.overflow = true;
                cpu.registers.p.negative = true;
                cpu.registers.stack_pointer = 0xFC;
                cpu.stack[0xFD] = 0b0011_0000;

                let wait_cycles = cpu.execute_instruction(0x28);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(cpu.registers.p.interrupt_disable, false);
                assert_eq!(cpu.registers.p.decimal, false);
                assert_eq!(cpu.registers.p.overflow, false);
                assert_eq!(cpu.registers.p.negative, false);
                assert_eq!(cpu.registers.stack_pointer, 0xFD);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_carry_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0xFC;
                cpu.stack[0xFD] = 0b0011_0001;

                let wait_cycles = cpu.execute_instruction(0x28);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(cpu.registers.p.interrupt_disable, false);
                assert_eq!(cpu.registers.p.decimal, false);
                assert_eq!(cpu.registers.p.overflow, false);
                assert_eq!(cpu.registers.p.negative, false);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0xFC;
                cpu.stack[0xFD] = 0b0011_0010;

                let wait_cycles = cpu.execute_instruction(0x28);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(cpu.registers.p.interrupt_disable, false);
                assert_eq!(cpu.registers.p.decimal, false);
                assert_eq!(cpu.registers.p.overflow, false);
                assert_eq!(cpu.registers.p.negative, false);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_interrupt_disable_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0xFC;
                cpu.stack[0xFD] = 0b0011_0100;

                let wait_cycles = cpu.execute_instruction(0x28);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(cpu.registers.p.interrupt_disable, true);
                assert_eq!(cpu.registers.p.decimal, false);
                assert_eq!(cpu.registers.p.overflow, false);
                assert_eq!(cpu.registers.p.negative, false);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_decimal_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0xFC;
                cpu.stack[0xFD] = 0b0011_1000;

                let wait_cycles = cpu.execute_instruction(0x28);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(cpu.registers.p.interrupt_disable, false);
                assert_eq!(cpu.registers.p.decimal, true);
                assert_eq!(cpu.registers.p.overflow, false);
                assert_eq!(cpu.registers.p.negative, false);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_overflow_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0xFC;
                cpu.stack[0xFD] = 0b0111_0000;

                let wait_cycles = cpu.execute_instruction(0x28);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(cpu.registers.p.interrupt_disable, false);
                assert_eq!(cpu.registers.p.decimal, false);
                assert_eq!(cpu.registers.p.overflow, true);
                assert_eq!(cpu.registers.p.negative, false);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0xFC;
                cpu.stack[0xFD] = 0b1011_0000;

                let wait_cycles = cpu.execute_instruction(0x28);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(cpu.registers.p.interrupt_disable, false);
                assert_eq!(cpu.registers.p.decimal, false);
                assert_eq!(cpu.registers.p.overflow, false);
                assert_eq!(cpu.registers.p.negative, true);
                assert_eq!(wait_cycles, 4);
            }
        }

        mod and
        {
            use super::*;

            #[test]
            fn test_immediate()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x29);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x29);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x29);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x29);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x0F;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x25);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x0F;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x25);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x0F;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x25);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x25);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x35);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x35);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x35);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x35);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x03] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x35);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x03] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x35);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x03] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x35);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x03] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x35);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x2D);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x2D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x2D);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x2D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x3D);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x3D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x3D);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x3D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x39);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x39);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x39);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x39);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x3D);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x3D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x3D);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x3D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x39);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x39);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x39);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x39);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_indirect()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x21);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x21);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x21);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x21);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_indirect_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x21);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x21);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x21);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x21);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indirect_indexed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x31);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x31);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x31);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x31);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indirect_indexed_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0304] = 0x0F;
                cpu.registers.y = 0x05;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x31);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0304] = 0x0F;
                cpu.registers.y = 0x05;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x31);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0304] = 0x0F;
                cpu.registers.y = 0x05;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x31);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0304] = 0x0F;
                cpu.registers.y = 0x05;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x31);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x00;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x29);

                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x29);

                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x80;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x29);

                assert_eq!(cpu.registers.p.negative, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x29);

                assert_eq!(cpu.registers.p.zero, false);
            }
        }

        mod ora
        {
            use super::*;

            #[test]
            fn test_immediate()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x09);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x09);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x09);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x09);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x0F;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x05);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x0F;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x05);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x0F;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x05);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x05);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x15);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x15);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x15);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x15);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x03] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x15);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x03] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x15);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x03] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x15);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x03] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x15);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x0D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x0D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x0D);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x0D);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x1D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x1D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x1D);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x1D);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);

                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x19);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x19);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x19);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x19);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x1D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x1D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x1D);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x1D);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 5);

                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x19);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x19);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x19);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x19);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_indirect()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x01);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x01);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x01);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x01);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_indirect_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x01);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x01);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x01);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x01);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indirect_indexed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x11);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x11);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x11);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x11);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indirect_indexed_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0304] = 0x0F;
                cpu.registers.y = 0x05;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x11);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0304] = 0x0F;
                cpu.registers.y = 0x05;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x11);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0304] = 0x0F;
                cpu.registers.y = 0x05;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x11);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0304] = 0x0F;
                cpu.registers.y = 0x05;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x11);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x00;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x09);

                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x09);

                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x80;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x09);

                assert_eq!(cpu.registers.p.negative, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x09);

                assert_eq!(cpu.registers.p.zero, false);
            }
        }

        mod eor
        {
            use super::*;

            #[test]
            fn test_immediate()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x49);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x49);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x49);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x49);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x0F;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x45);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x0F;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x45);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x0F;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x45);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x45);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x55);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x55);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x55);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x55);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x03] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x55);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x03] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x55);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x03] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x55);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x03] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x55);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x4D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x4D);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x4D);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x4D);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x5D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x5D);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x5D);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x5D);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 4);

                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x59);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x59);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x59);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x59);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x5D);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x5D);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x5D);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.x = 0x04;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x5D);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 5);

                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x59);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x59);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x59);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0303] = 0x0F;
                cpu.registers.y = 0x04;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x59);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_indirect()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x41);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x41);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x41);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x41);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_indirect_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x41);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x41);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x41);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x0F;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x41);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indirect_indexed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x51);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x51);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x51);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x0F;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x51);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indirect_indexed_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0304] = 0x0F;
                cpu.registers.y = 0x05;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x51);

                assert_eq!(cpu.registers.a, 0x0F);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0304] = 0x0F;
                cpu.registers.y = 0x05;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x51);

                assert_eq!(cpu.registers.a, 0x00);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0304] = 0x0F;
                cpu.registers.y = 0x05;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x51);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0304] = 0x0F;
                cpu.registers.y = 0x05;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x51);

                assert_eq!(cpu.registers.a, 0xF0);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x00;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x49);

                assert_eq!(cpu.registers.p.zero, true);

                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x49);

                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x49);

                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xF0;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x49);

                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x80;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x09);

                assert_eq!(cpu.registers.p.negative, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x0F;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x09);

                assert_eq!(cpu.registers.p.zero, false);
            }
        }

        mod bit
        {
            use super::*;

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x24);

                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x01;
                cpu.registers.a = 0x01;

                let wait_cycles = cpu.execute_instruction(0x24);

                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x24);

                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x24);

                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x01;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x2C);

                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x01;
                cpu.registers.a = 0x01;

                let wait_cycles = cpu.execute_instruction(0x2C);

                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x2C);

                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x01;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0x2C);

                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x24);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x81;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x24);

                assert_eq!(cpu.registers.p.negative, true);
            }

            #[test]
            fn test_overflow_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x01;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x24);

                assert_eq!(cpu.registers.p.overflow, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x41;
                cpu.registers.a = 0x0F;

                let wait_cycles = cpu.execute_instruction(0x24);

                assert_eq!(cpu.registers.p.overflow, true);
            }
        }

        mod adc
        {
            use super::*;

            #[test]
            fn test_immediate()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x69);

                assert_eq!(cpu.registers.a, 0x05);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x69);

                assert_eq!(cpu.registers.a, 0x06);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x06;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x65);

                assert_eq!(cpu.registers.a, 0x07);
                assert_eq!(wait_cycles, 3);


                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x06;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x65);

                assert_eq!(cpu.registers.a, 0x08);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0xFD;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x75);

                assert_eq!(cpu.registers.a, 0xFE);
                assert_eq!(wait_cycles, 4);

                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0xFD;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x75);

                assert_eq!(cpu.registers.a, 0xFF);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x00] = 0x04;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x75);

                assert_eq!(cpu.registers.a, 0x05);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x00] = 0x04;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x75);

                assert_eq!(cpu.registers.a, 0x06);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x1F; // same as 0x07 due to mirroring
                cpu.internal_ram[0x0504] = 0x06;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x6D);

                assert_eq!(cpu.registers.a, 0x07);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x1F; // same as 0x07 due to mirroring
                cpu.internal_ram[0x0504] = 0x06;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x6D);

                assert_eq!(cpu.registers.a, 0x08);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x06;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x7D);

                assert_eq!(cpu.registers.a, 0x07);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x06;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x79);

                assert_eq!(cpu.registers.a, 0x07);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x06;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x7D);

                assert_eq!(cpu.registers.a, 0x08);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x06;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x79);

                assert_eq!(cpu.registers.a, 0x08);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x06;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x7D);

                assert_eq!(cpu.registers.a, 0x07);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x06;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x79);

                assert_eq!(cpu.registers.a, 0x07);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x06;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x7D);

                assert_eq!(cpu.registers.a, 0x08);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x06;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x79);

                assert_eq!(cpu.registers.a, 0x08);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_indirect()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x07] = 0x04;
                cpu.zero_page_ram[0x08] = 0x06;
                cpu.internal_ram[0x0404] = 0x07;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x61);

                assert_eq!(cpu.registers.a, 0x08);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x07] = 0x04;
                cpu.zero_page_ram[0x08] = 0x06;
                cpu.internal_ram[0x0404] = 0x07;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x61);

                assert_eq!(cpu.registers.a, 0x09);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_indirect_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x07] = 0x04;
                cpu.zero_page_ram[0x08] = 0x06;
                cpu.internal_ram[0x0404] = 0x07;
                cpu.registers.x = 0x08;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x61);

                assert_eq!(cpu.registers.a, 0x08);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x07] = 0x04;
                cpu.zero_page_ram[0x08] = 0x06;
                cpu.internal_ram[0x0404] = 0x07;
                cpu.registers.x = 0x08;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x61);

                assert_eq!(cpu.registers.a, 0x09);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indirect_indexed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x06;
                cpu.internal_ram[0x0407] = 0x07;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x71);

                assert_eq!(cpu.registers.a, 0x08);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x06;
                cpu.internal_ram[0x0407] = 0x07;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x71);

                assert_eq!(cpu.registers.a, 0x09);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indirect_indexed_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x06;
                cpu.internal_ram[0x0502] = 0x07;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x71);

                assert_eq!(cpu.registers.a, 0x08);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x06;
                cpu.internal_ram[0x0502] = 0x07;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x71);

                assert_eq!(cpu.registers.a, 0x09);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_carry_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                cpu.execute_instruction(0x69);
                assert_eq!(cpu.registers.p.carry, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                cpu.execute_instruction(0x69);
                assert_eq!(cpu.registers.p.carry, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0xFF;
                cpu.registers.p.carry = false;

                cpu.execute_instruction(0x69);
                assert_eq!(cpu.registers.p.carry, true);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x01;
                cpu.registers.a = 0xFF;
                cpu.registers.p.carry = false;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0x69);
                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0xFF;
                cpu.registers.p.carry = false;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0x69);
                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_overflow_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;
                cpu.registers.p.overflow = false;

                cpu.execute_instruction(0x69);
                assert_eq!(cpu.registers.p.overflow, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x40;
                cpu.registers.a = 0x40;
                cpu.registers.p.carry = false;
                cpu.registers.p.overflow = false;

                cpu.execute_instruction(0x69);
                assert_eq!(cpu.registers.p.overflow, true);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0x69);
                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x80;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0x69);
                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod sbc
        {
            use super::*;

            #[test]
            fn test_immediate()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x01;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xE9);

                assert_eq!(cpu.registers.a, 0x02);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x01;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xE9);

                assert_eq!(cpu.registers.a, 0x03);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x01;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xE5);

                assert_eq!(cpu.registers.a, 0x04);
                assert_eq!(wait_cycles, 3);


                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x01;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xE5);

                assert_eq!(cpu.registers.a, 0x05);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x01;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xFD;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xF5);

                assert_eq!(cpu.registers.a, 0xFB);
                assert_eq!(wait_cycles, 4);

                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x01;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0xFD;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xF5);

                assert_eq!(cpu.registers.a, 0xFC);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x00] = 0x01;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xF5);

                assert_eq!(cpu.registers.a, 0x02);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x00] = 0x01;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xF5);

                assert_eq!(cpu.registers.a, 0x03);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x1F; // same as 0x07 due to mirroring
                cpu.internal_ram[0x0504] = 0x01;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xED);

                assert_eq!(cpu.registers.a, 0x04);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x1F; // same as 0x07 due to mirroring
                cpu.internal_ram[0x0504] = 0x01;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xED);

                assert_eq!(cpu.registers.a, 0x05);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x01;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xFD);

                assert_eq!(cpu.registers.a, 0x04);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x01;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xF9);

                assert_eq!(cpu.registers.a, 0x04);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x01;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xFD);

                assert_eq!(cpu.registers.a, 0x05);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x01;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xF9);

                assert_eq!(cpu.registers.a, 0x05);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x01;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xFD);

                assert_eq!(cpu.registers.a, 0x04);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x01;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xF9);

                assert_eq!(cpu.registers.a, 0x04);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x01;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xFD);

                assert_eq!(cpu.registers.a, 0x05);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0302] = 0x01;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xF9);

                assert_eq!(cpu.registers.a, 0x05);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_indirect()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x07] = 0x04;
                cpu.zero_page_ram[0x08] = 0x06;
                cpu.internal_ram[0x0404] = 0x01;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x07;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xE1);

                assert_eq!(cpu.registers.a, 0x05);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x07] = 0x04;
                cpu.zero_page_ram[0x08] = 0x06;
                cpu.internal_ram[0x0404] = 0x01;
                cpu.registers.x = 0x03;
                cpu.registers.a = 0x07;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xE1);

                assert_eq!(cpu.registers.a, 0x06);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_indirect_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x07] = 0x04;
                cpu.zero_page_ram[0x08] = 0x06;
                cpu.internal_ram[0x0404] = 0x01;
                cpu.registers.x = 0x08;
                cpu.registers.a = 0x07;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xE1);

                assert_eq!(cpu.registers.a, 0x05);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x07] = 0x04;
                cpu.zero_page_ram[0x08] = 0x06;
                cpu.internal_ram[0x0404] = 0x01;
                cpu.registers.x = 0x08;
                cpu.registers.a = 0x07;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xE1);

                assert_eq!(cpu.registers.a, 0x06);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indirect_indexed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x06;
                cpu.internal_ram[0x0407] = 0x01;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x07;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xF1);

                assert_eq!(cpu.registers.a, 0x05);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x06;
                cpu.internal_ram[0x0407] = 0x01;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x07;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xF1);

                assert_eq!(cpu.registers.a, 0x06);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indirect_indexed_page_boundaries_crossed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x06;
                cpu.internal_ram[0x0502] = 0x01;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x07;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xF1);

                assert_eq!(cpu.registers.a, 0x05);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x06;
                cpu.internal_ram[0x0502] = 0x01;
                cpu.registers.y = 0x03;
                cpu.registers.a = 0x07;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xF1);

                assert_eq!(cpu.registers.a, 0x06);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_carry_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                cpu.execute_instruction(0xE9);
                assert_eq!(cpu.registers.p.carry, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0xFF;
                cpu.registers.p.carry = true;

                cpu.execute_instruction(0xE9);
                assert_eq!(cpu.registers.p.carry, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0xFF;
                cpu.registers.p.carry = false;

                cpu.execute_instruction(0xE9);
                assert_eq!(cpu.registers.p.carry, true);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.registers.a = 0xFF;
                cpu.registers.p.carry = true;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xE9);
                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0xFF;
                cpu.registers.p.carry = false;
                cpu.registers.p.zero = false;

                cpu.execute_instruction(0xE9);
                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_overflow_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0x06;
                cpu.registers.p.carry = true;
                cpu.registers.p.overflow = false;

                cpu.execute_instruction(0xE9);
                assert_eq!(cpu.registers.p.overflow, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x80;
                cpu.registers.a = 0x80;
                cpu.registers.p.carry = true;
                cpu.registers.p.overflow = false;

                cpu.execute_instruction(0xE9);
                assert_eq!(cpu.registers.p.overflow, true);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0x08;
                cpu.registers.p.carry = true;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xE9);
                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.registers.a = 0x88;
                cpu.registers.p.carry = false;
                cpu.registers.p.negative = false;

                cpu.execute_instruction(0xE9);
                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod cmp
        {
            use super::*;

            #[test]
            fn test_immediate()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x05;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xC9);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x05;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xC9);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x05;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xC9);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x05;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xC5);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x05;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xC5);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x05;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xC5);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x05;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xD5);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x05;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xD5);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x05;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xD5);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x05;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xD5);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x05;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xD5);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x05;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xD5);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xCD);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xCD);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xCD);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x05;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xDD);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x05;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xDD);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x05;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xDD);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);

                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x05;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xD9);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x05;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xD9);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x05;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xD9);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_indexed_absolute_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0305] = 0x05;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xDD);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0305] = 0x05;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xDD);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0305] = 0x05;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xDD);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 5);

                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0305] = 0x05;
                cpu.registers.y = 0x06;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xD9);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0305] = 0x05;
                cpu.registers.y = 0x06;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xD9);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0305] = 0x05;
                cpu.registers.y = 0x06;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xD9);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_indirect()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xC1);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xC1);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.x = 0x01;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xC1);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_indirect_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xC1);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xC1);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.x = 0x06;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xC1);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indirect_indexed()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x05;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xD1);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0205] = 0x05;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xD1);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.internal_ram[0x0205] = 0x05;
                cpu.registers.y = 0x01;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xD1);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indirect_indexed_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0305] = 0x05;
                cpu.registers.y = 0x06;
                cpu.registers.a = 0x06;

                let wait_cycles = cpu.execute_instruction(0xD1);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0305] = 0x05;
                cpu.registers.y = 0x06;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xD1);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.internal_ram[0x0305] = 0x05;
                cpu.registers.y = 0x06;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0xD1);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_negative_flags()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x70;
                cpu.registers.a = 0xF0;

                let wait_cycles = cpu.execute_instruction(0xC9);

                assert_eq!(cpu.registers.p.negative, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x05;
                cpu.registers.a = 0x05;

                let wait_cycles = cpu.execute_instruction(0xC9);

                assert_eq!(cpu.registers.p.negative, false);
            }
        }

        mod cpx
        {
            use super::*;

            #[test]
            fn test_immediate()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x05;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0xE0);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x05;
                cpu.registers.x = 0x05;

                let wait_cycles = cpu.execute_instruction(0xE0);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x05;
                cpu.registers.x = 0x04;

                let wait_cycles = cpu.execute_instruction(0xE0);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x05;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0xE4);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x05;
                cpu.registers.x = 0x05;

                let wait_cycles = cpu.execute_instruction(0xE4);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x05;
                cpu.registers.x = 0x04;

                let wait_cycles = cpu.execute_instruction(0xE4);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0xEC);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.x = 0x05;

                let wait_cycles = cpu.execute_instruction(0xEC);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.x = 0x04;

                let wait_cycles = cpu.execute_instruction(0xEC);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_negative_flags()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x70;
                cpu.registers.x = 0xF0;

                let wait_cycles = cpu.execute_instruction(0xE0);

                assert_eq!(cpu.registers.p.negative, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x05;
                cpu.registers.x = 0x05;

                let wait_cycles = cpu.execute_instruction(0xE0);

                assert_eq!(cpu.registers.p.negative, false);
            }
        }

        mod cpy
        {
            use super::*;

            #[test]
            fn test_immediate()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x05;
                cpu.registers.y = 0x06;

                let wait_cycles = cpu.execute_instruction(0xC0);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x05;
                cpu.registers.y = 0x05;

                let wait_cycles = cpu.execute_instruction(0xC0);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x05;
                cpu.registers.y = 0x04;

                let wait_cycles = cpu.execute_instruction(0xC0);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x05;
                cpu.registers.y = 0x06;

                let wait_cycles = cpu.execute_instruction(0xC4);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x05;
                cpu.registers.y = 0x05;

                let wait_cycles = cpu.execute_instruction(0xC4);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x05;
                cpu.registers.y = 0x04;

                let wait_cycles = cpu.execute_instruction(0xC4);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.y = 0x06;

                let wait_cycles = cpu.execute_instruction(0xCC);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.y = 0x05;

                let wait_cycles = cpu.execute_instruction(0xCC);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, true);
                assert_eq!(wait_cycles, 4);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;
                cpu.registers.y = 0x04;

                let wait_cycles = cpu.execute_instruction(0xCC);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_negative_flags()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x70;
                cpu.registers.y = 0xF0;

                let wait_cycles = cpu.execute_instruction(0xC0);

                assert_eq!(cpu.registers.p.negative, true);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x05;
                cpu.registers.y = 0x05;

                let wait_cycles = cpu.execute_instruction(0xC0);

                assert_eq!(cpu.registers.p.negative, false);
            }
        }

        mod inc
        {
            use super::*;

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;

                let wait_cycles = cpu.execute_instruction(0xE6);

                assert_eq!(cpu.zero_page_ram[0x04], 0x05);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;

                let wait_cycles = cpu.execute_instruction(0xE6);

                assert_eq!(cpu.zero_page_ram[0x04], 0x00);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x05;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0xF6);

                assert_eq!(cpu.zero_page_ram[0x05], 0x06);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0xFF;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0xF6);

                assert_eq!(cpu.zero_page_ram[0x05], 0x00);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0xF6);

                assert_eq!(cpu.zero_page_ram[0x05], 0x05);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0xFF;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0xF6);

                assert_eq!(cpu.zero_page_ram[0x05], 0x00);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;

                let wait_cycles = cpu.execute_instruction(0xEE);

                assert_eq!(cpu.internal_ram[0x0204], 0x06);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0xFF;

                let wait_cycles = cpu.execute_instruction(0xEE);

                assert_eq!(cpu.internal_ram[0x0204], 0x00);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x05;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0xFE);

                assert_eq!(cpu.internal_ram[0x0205], 0x06);
                assert_eq!(wait_cycles, 7);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0xFF;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0xFE);

                assert_eq!(cpu.internal_ram[0x0205], 0x00);
                assert_eq!(wait_cycles, 7);
            }

            #[test]
            fn test_indexed_absolute_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0305] = 0x05;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0xFE);

                assert_eq!(cpu.internal_ram[0x0305], 0x06);
                assert_eq!(wait_cycles, 7);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0305] = 0xFF;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0xFE);

                assert_eq!(cpu.internal_ram[0x0305], 0x00);
                assert_eq!(wait_cycles, 7);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;

                let wait_cycles = cpu.execute_instruction(0xE6);

                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0xFF;

                let wait_cycles = cpu.execute_instruction(0xE6);

                assert_eq!(cpu.registers.p.zero, true);
            }


            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x7E;

                let wait_cycles = cpu.execute_instruction(0xE6);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x7F;

                let wait_cycles = cpu.execute_instruction(0xE6);

                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod inx
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x04;

                let wait_cycles = cpu.execute_instruction(0xE8);

                assert_eq!(cpu.registers.x, 0x05);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0xFF;

                let wait_cycles = cpu.execute_instruction(0xE8);

                assert_eq!(cpu.registers.x, 0x00);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x04;

                let wait_cycles = cpu.execute_instruction(0xE8);

                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0xFF;

                let wait_cycles = cpu.execute_instruction(0xE8);

                assert_eq!(cpu.registers.p.zero, true);
            }


            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x04;

                let wait_cycles = cpu.execute_instruction(0xE8);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x7F;

                let wait_cycles = cpu.execute_instruction(0xE8);

                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod iny
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x04;

                let wait_cycles = cpu.execute_instruction(0xC8);

                assert_eq!(cpu.registers.y, 0x05);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0xFF;

                let wait_cycles = cpu.execute_instruction(0xC8);

                assert_eq!(cpu.registers.y, 0x00);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x04;

                let wait_cycles = cpu.execute_instruction(0xC8);

                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0xFF;

                let wait_cycles = cpu.execute_instruction(0xC8);

                assert_eq!(cpu.registers.p.zero, true);
            }


            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x04;

                let wait_cycles = cpu.execute_instruction(0xC8);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x7F;

                let wait_cycles = cpu.execute_instruction(0xC8);

                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod dec
        {
            use super::*;

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;

                let wait_cycles = cpu.execute_instruction(0xC6);

                assert_eq!(cpu.zero_page_ram[0x04], 0x03);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x00;

                let wait_cycles = cpu.execute_instruction(0xC6);

                assert_eq!(cpu.zero_page_ram[0x04], 0xFF);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x05;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0xD6);

                assert_eq!(cpu.zero_page_ram[0x05], 0x04);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x05] = 0x00;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0xD6);

                assert_eq!(cpu.zero_page_ram[0x05], 0xFF);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0xD6);

                assert_eq!(cpu.zero_page_ram[0x05], 0x03);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x00;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0xD6);

                assert_eq!(cpu.zero_page_ram[0x05], 0xFF);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x05;

                let wait_cycles = cpu.execute_instruction(0xCE);

                assert_eq!(cpu.internal_ram[0x0204], 0x04);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0204] = 0x00;

                let wait_cycles = cpu.execute_instruction(0xCE);

                assert_eq!(cpu.internal_ram[0x0204], 0xFF);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x05;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0xDE);

                assert_eq!(cpu.internal_ram[0x0205], 0x04);
                assert_eq!(wait_cycles, 7);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0205] = 0x00;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0xDE);

                assert_eq!(cpu.internal_ram[0x0205], 0xFF);
                assert_eq!(wait_cycles, 7);
            }

            #[test]
            fn test_indexed_absolute_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0305] = 0x05;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0xDE);

                assert_eq!(cpu.internal_ram[0x0305], 0x04);
                assert_eq!(wait_cycles, 7);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0xFF;
                cpu.internal_ram[1] = 0x04;
                cpu.internal_ram[0x0305] = 0x00;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0xDE);

                assert_eq!(cpu.internal_ram[0x0305], 0xFF);
                assert_eq!(wait_cycles, 7);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x04;

                let wait_cycles = cpu.execute_instruction(0xC6);

                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x01;

                let wait_cycles = cpu.execute_instruction(0xC6);

                assert_eq!(cpu.registers.p.zero, true);
            }


            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x80;

                let wait_cycles = cpu.execute_instruction(0xC6);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0] = 0x04;
                cpu.zero_page_ram[0x04] = 0x81;

                let wait_cycles = cpu.execute_instruction(0xC6);

                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod dex
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x04;

                let wait_cycles = cpu.execute_instruction(0xCA);

                assert_eq!(cpu.registers.x, 0x03);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x00;

                let wait_cycles = cpu.execute_instruction(0xCA);

                assert_eq!(cpu.registers.x, 0xFF);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x04;

                let wait_cycles = cpu.execute_instruction(0xCA);

                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0xCA);

                assert_eq!(cpu.registers.p.zero, true);
            }


            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x80;

                let wait_cycles = cpu.execute_instruction(0xCA);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.x = 0x81;

                let wait_cycles = cpu.execute_instruction(0xCA);

                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod dey
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x04;

                let wait_cycles = cpu.execute_instruction(0x88);

                assert_eq!(cpu.registers.y, 0x03);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x00;

                let wait_cycles = cpu.execute_instruction(0x88);

                assert_eq!(cpu.registers.y, 0xFF);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x04;

                let wait_cycles = cpu.execute_instruction(0x88);

                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x01;

                let wait_cycles = cpu.execute_instruction(0x88);

                assert_eq!(cpu.registers.p.zero, true);
            }


            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x80;

                let wait_cycles = cpu.execute_instruction(0x88);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.y = 0x81;

                let wait_cycles = cpu.execute_instruction(0x88);

                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod asl
        {
            use super::*;

            #[test]
            fn test_accumulator()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0x0A);

                assert_eq!(cpu.registers.a, 0x08);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x05;
                cpu.zero_page_ram[0x05] = 0x04;

                let wait_cycles = cpu.execute_instruction(0x06);

                assert_eq!(cpu.zero_page_ram[0x05], 0x08);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x05;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0x16);

                assert_eq!(cpu.zero_page_ram[0x06], 0x08);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0x16);

                assert_eq!(cpu.zero_page_ram[0x05], 0x08);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0204] = 0x04;

                let wait_cycles = cpu.execute_instruction(0x0E);

                assert_eq!(cpu.internal_ram[0x0204], 0x08);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0205] = 0x04;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0x1E);

                assert_eq!(cpu.internal_ram[0x0205], 0x08);
                assert_eq!(wait_cycles, 7);
            }

            #[test]
            fn test_indexed_absolute_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x0FF;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0305] = 0x04;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0x1E);

                assert_eq!(cpu.internal_ram[0x0305], 0x08);
                assert_eq!(wait_cycles, 7);
            }

            #[test]
            fn test_carry_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0x0A);

                assert_eq!(cpu.registers.p.carry, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x0A);

                assert_eq!(cpu.registers.p.carry, true);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0x0A);

                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x80;

                let wait_cycles = cpu.execute_instruction(0x0A);

                assert_eq!(cpu.registers.p.zero, true);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0x0A);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x40;

                let wait_cycles = cpu.execute_instruction(0x0A);

                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod lsr
        {
            use super::*;

            #[test]
            fn test_accumulator()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0x4A);

                assert_eq!(cpu.registers.a, 0x02);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x05;
                cpu.zero_page_ram[0x05] = 0x04;

                let wait_cycles = cpu.execute_instruction(0x46);

                assert_eq!(cpu.zero_page_ram[0x05], 0x02);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x05;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0x56);

                assert_eq!(cpu.zero_page_ram[0x06], 0x02);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0x56);

                assert_eq!(cpu.zero_page_ram[0x05], 0x02);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0204] = 0x04;

                let wait_cycles = cpu.execute_instruction(0x4E);

                assert_eq!(cpu.internal_ram[0x0204], 0x02);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0205] = 0x04;
                cpu.registers.x = 0x01;

                let wait_cycles = cpu.execute_instruction(0x5E);

                assert_eq!(cpu.internal_ram[0x0205], 0x02);
                assert_eq!(wait_cycles, 7);
            }

            #[test]
            fn test_indexed_absolute_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x0FF;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0305] = 0x04;
                cpu.registers.x = 0x06;

                let wait_cycles = cpu.execute_instruction(0x5E);

                assert_eq!(cpu.internal_ram[0x0305], 0x02);
                assert_eq!(wait_cycles, 7);
            }

            #[test]
            fn test_carry_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0x4A);

                assert_eq!(cpu.registers.p.carry, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x4A);

                assert_eq!(cpu.registers.p.carry, true);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0x4A);

                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x01;

                let wait_cycles = cpu.execute_instruction(0x4A);

                assert_eq!(cpu.registers.p.zero, true);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;

                let wait_cycles = cpu.execute_instruction(0x4A);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0xFF;

                let wait_cycles = cpu.execute_instruction(0x4A);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x00;

                let wait_cycles = cpu.execute_instruction(0x4A);

                assert_eq!(cpu.registers.p.negative, false);
            }
        }

        mod rol
        {
            use super::*;

            #[test]
            fn test_accumulator()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x2A);

                assert_eq!(cpu.registers.a, 0x08);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x2A);

                assert_eq!(cpu.registers.a, 0x09);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x05;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x26);

                assert_eq!(cpu.zero_page_ram[0x05], 0x08);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x05;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x26);

                assert_eq!(cpu.zero_page_ram[0x05], 0x09);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x05;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.registers.x = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x36);

                assert_eq!(cpu.zero_page_ram[0x06], 0x08);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x05;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.registers.x = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x36);

                assert_eq!(cpu.zero_page_ram[0x06], 0x09);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.registers.x = 0x06;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x36);

                assert_eq!(cpu.zero_page_ram[0x05], 0x08);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.registers.x = 0x06;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x36);

                assert_eq!(cpu.zero_page_ram[0x05], 0x09);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0204] = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x2E);

                assert_eq!(cpu.internal_ram[0x0204], 0x08);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0204] = 0x04;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x2E);

                assert_eq!(cpu.internal_ram[0x0204], 0x09);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0205] = 0x04;
                cpu.registers.x = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x3E);

                assert_eq!(cpu.internal_ram[0x0205], 0x08);
                assert_eq!(wait_cycles, 7);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0205] = 0x04;
                cpu.registers.x = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x3E);

                assert_eq!(cpu.internal_ram[0x0205], 0x09);
                assert_eq!(wait_cycles, 7);
            }

            #[test]
            fn test_indexed_absolute_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x0FF;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0305] = 0x04;
                cpu.registers.x = 0x06;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x3E);

                assert_eq!(cpu.internal_ram[0x0305], 0x08);
                assert_eq!(wait_cycles, 7);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x0FF;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0305] = 0x04;
                cpu.registers.x = 0x06;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x3E);

                assert_eq!(cpu.internal_ram[0x0305], 0x09);
                assert_eq!(wait_cycles, 7);
            }

            #[test]
            fn test_carry_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x2A);

                assert_eq!(cpu.registers.p.carry, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0xFF;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x2A);

                assert_eq!(cpu.registers.p.carry, true);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x2A);

                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x80;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x2A);

                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x80;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x2A);

                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x2A);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x40;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x2A);

                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod ror
        {
            use super::*;

            #[test]
            fn test_accumulator()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.a, 0x02);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.a, 0x82);
                assert_eq!(wait_cycles, 2);
            }

            #[test]
            fn test_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x05;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x66);

                assert_eq!(cpu.zero_page_ram[0x05], 0x02);
                assert_eq!(wait_cycles, 5);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x05;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x66);

                assert_eq!(cpu.zero_page_ram[0x05], 0x82);
                assert_eq!(wait_cycles, 5);
            }

            #[test]
            fn test_indexed_zero_page()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x05;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.registers.x = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x76);

                assert_eq!(cpu.zero_page_ram[0x06], 0x02);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x05;
                cpu.zero_page_ram[0x06] = 0x04;
                cpu.registers.x = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x76);

                assert_eq!(cpu.zero_page_ram[0x06], 0x82);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_zero_page_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.registers.x = 0x06;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x76);

                assert_eq!(cpu.zero_page_ram[0x05], 0x02);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0xFF;
                cpu.zero_page_ram[0x05] = 0x04;
                cpu.registers.x = 0x06;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x76);

                assert_eq!(cpu.zero_page_ram[0x05], 0x82);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0204] = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x6E);

                assert_eq!(cpu.internal_ram[0x0204], 0x02);
                assert_eq!(wait_cycles, 6);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0204] = 0x04;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x6E);

                assert_eq!(cpu.internal_ram[0x0204], 0x82);
                assert_eq!(wait_cycles, 6);
            }

            #[test]
            fn test_indexed_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0205] = 0x04;
                cpu.registers.x = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x7E);

                assert_eq!(cpu.internal_ram[0x0205], 0x02);
                assert_eq!(wait_cycles, 7);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0205] = 0x04;
                cpu.registers.x = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x7E);

                assert_eq!(cpu.internal_ram[0x0205], 0x82);
                assert_eq!(wait_cycles, 7);
            }

            #[test]
            fn test_indexed_absolute_crossing_page_boundaries()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x0FF;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0305] = 0x04;
                cpu.registers.x = 0x06;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x7E);

                assert_eq!(cpu.internal_ram[0x0305], 0x02);
                assert_eq!(wait_cycles, 7);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x0FF;
                cpu.internal_ram[0x01] = 0x04;
                cpu.internal_ram[0x0305] = 0x04;
                cpu.registers.x = 0x06;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x7E);

                assert_eq!(cpu.internal_ram[0x0305], 0x82);
                assert_eq!(wait_cycles, 7);
            }

            #[test]
            fn test_carry_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.p.carry, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0xFF;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.p.carry, true);
            }

            #[test]
            fn test_zero_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.p.zero, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.p.zero, true);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x01;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.p.zero, false);
            }

            #[test]
            fn test_negative_flag()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0xFF;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x00;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.p.negative, false);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x04;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.p.negative, true);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0xFF;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.p.negative, true);

                cpu.registers.pc = 0x0200;
                cpu.registers.a = 0x00;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x6A);

                assert_eq!(cpu.registers.p.negative, true);
            }
        }

        mod jmp
        {
            use super::*;

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x40;
                cpu.internal_ram[0x01] = 0x04;

                let wait_cycles = cpu.execute_instruction(0x4C);

                assert_eq!(cpu.registers.pc, 0x0440);
                assert_eq!(wait_cycles, 3);
            }

            #[test]
            fn test_indirect()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x50;
                cpu.internal_ram[0x01] = 0x05;
                cpu.internal_ram[0x0350] = 0x40;
                cpu.internal_ram[0x0351] = 0x04;

                let wait_cycles = cpu.execute_instruction(0x6C);

                assert_eq!(cpu.registers.pc, 0x0440);
                assert_eq!(wait_cycles, 5);
            }
        }

        mod jsr
        {
            use super::*;

            #[test]
            fn test_absolute()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x40;
                cpu.internal_ram[0x01] = 0x04;
                cpu.registers.stack_pointer = 0xFD;
                cpu.stack[0xFD] = 0x04;
                cpu.stack[0xFC] = 0x04;
                // pc should be 0x0202 here

                let wait_cycles = cpu.execute_instruction(0x20);

                assert_eq!(cpu.registers.pc, 0x0440);
                assert_eq!(cpu.registers.stack_pointer, 0xFB);
                assert_eq!(cpu.stack[0xFD], 0x02);
                assert_eq!(cpu.stack[0xFC], 0x01);
                assert_eq!(wait_cycles, 6);
            }
        }

        mod rts
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0xFB;
                cpu.stack[0xFD] = 0x04;
                cpu.stack[0xFC] = 0x40;
                // pc should be 0x0202 here

                let wait_cycles = cpu.execute_instruction(0x60);

                assert_eq!(cpu.registers.pc, 0x0441);
                assert_eq!(cpu.registers.stack_pointer, 0xFD);
                assert_eq!(wait_cycles, 6);
            }
        }

        mod bcc
        {
            use super::*;

            #[test]
            fn test_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x90);

                assert_eq!(cpu.registers.pc, 0x0201);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x90);

                assert_eq!(cpu.registers.pc, 0x0205);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x02FE;
                cpu.internal_ram[0xFE] = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x90);

                assert_eq!(cpu.registers.pc, 0x0303);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_negative_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-4 as i8) as u8;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x90);

                assert_eq!(cpu.registers.pc, 0x0200);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-5 as i8) as u8;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x90);

                assert_eq!(cpu.registers.pc, 0x01FF);
                assert_eq!(wait_cycles, 4);
            }
        }

        mod bcs
        {
            use super::*;

            #[test]
            fn test_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0xB0);

                assert_eq!(cpu.registers.pc, 0x0201);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xB0);

                assert_eq!(cpu.registers.pc, 0x0205);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x02FE;
                cpu.internal_ram[0xFE] = 0x04;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xB0);

                assert_eq!(cpu.registers.pc, 0x0303);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_negative_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-4 as i8) as u8;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xB0);

                assert_eq!(cpu.registers.pc, 0x0200);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-5 as i8) as u8;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xB0);

                assert_eq!(cpu.registers.pc, 0x01FF);
                assert_eq!(wait_cycles, 4);
            }
        }

        mod beq
        {
            use super::*;

            #[test]
            fn test_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.zero = false;

                let wait_cycles = cpu.execute_instruction(0xF0);

                assert_eq!(cpu.registers.pc, 0x0201);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.zero = true;

                let wait_cycles = cpu.execute_instruction(0xF0);

                assert_eq!(cpu.registers.pc, 0x0205);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x02FE;
                cpu.internal_ram[0xFE] = 0x04;
                cpu.registers.p.zero = true;

                let wait_cycles = cpu.execute_instruction(0xF0);

                assert_eq!(cpu.registers.pc, 0x0303);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_negative_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-4 as i8) as u8;
                cpu.registers.p.zero = true;

                let wait_cycles = cpu.execute_instruction(0xF0);

                assert_eq!(cpu.registers.pc, 0x0200);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-5 as i8) as u8;
                cpu.registers.p.zero = true;

                let wait_cycles = cpu.execute_instruction(0xF0);

                assert_eq!(cpu.registers.pc, 0x01FF);
                assert_eq!(wait_cycles, 4);
            }
        }

        mod bne
        {
            use super::*;

            #[test]
            fn test_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.zero = true;

                let wait_cycles = cpu.execute_instruction(0xD0);

                assert_eq!(cpu.registers.pc, 0x0201);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.zero = false;

                let wait_cycles = cpu.execute_instruction(0xD0);

                assert_eq!(cpu.registers.pc, 0x0205);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x02FE;
                cpu.internal_ram[0xFE] = 0x04;
                cpu.registers.p.zero = false;

                let wait_cycles = cpu.execute_instruction(0xD0);

                assert_eq!(cpu.registers.pc, 0x0303);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_negative_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-4 as i8) as u8;
                cpu.registers.p.zero = false;

                let wait_cycles = cpu.execute_instruction(0xD0);

                assert_eq!(cpu.registers.pc, 0x0200);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-5 as i8) as u8;
                cpu.registers.p.zero = false;

                let wait_cycles = cpu.execute_instruction(0xD0);

                assert_eq!(cpu.registers.pc, 0x01FF);
                assert_eq!(wait_cycles, 4);
            }
        }

        mod bmi
        {
            use super::*;

            #[test]
            fn test_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.negative = false;

                let wait_cycles = cpu.execute_instruction(0x30);

                assert_eq!(cpu.registers.pc, 0x0201);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.negative = true;

                let wait_cycles = cpu.execute_instruction(0x30);

                assert_eq!(cpu.registers.pc, 0x0205);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x02FE;
                cpu.internal_ram[0xFE] = 0x04;
                cpu.registers.p.negative = true;

                let wait_cycles = cpu.execute_instruction(0x30);

                assert_eq!(cpu.registers.pc, 0x0303);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_negative_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-4 as i8) as u8;
                cpu.registers.p.negative = true;

                let wait_cycles = cpu.execute_instruction(0x30);

                assert_eq!(cpu.registers.pc, 0x0200);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-5 as i8) as u8;
                cpu.registers.p.negative = true;

                let wait_cycles = cpu.execute_instruction(0x30);

                assert_eq!(cpu.registers.pc, 0x01FF);
                assert_eq!(wait_cycles, 4);
            }
        }

        mod bpl
        {
            use super::*;

            #[test]
            fn test_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.negative = true;

                let wait_cycles = cpu.execute_instruction(0x10);

                assert_eq!(cpu.registers.pc, 0x0201);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.negative = false;

                let wait_cycles = cpu.execute_instruction(0x10);

                assert_eq!(cpu.registers.pc, 0x0205);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x02FE;
                cpu.internal_ram[0xFE] = 0x04;
                cpu.registers.p.negative = false;

                let wait_cycles = cpu.execute_instruction(0x10);

                assert_eq!(cpu.registers.pc, 0x0303);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_negative_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-4 as i8) as u8;
                cpu.registers.p.negative = false;

                let wait_cycles = cpu.execute_instruction(0x10);

                assert_eq!(cpu.registers.pc, 0x0200);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-5 as i8) as u8;
                cpu.registers.p.negative = false;

                let wait_cycles = cpu.execute_instruction(0x10);

                assert_eq!(cpu.registers.pc, 0x01FF);
                assert_eq!(wait_cycles, 4);
            }
        }

        mod bvs
        {
            use super::*;

            #[test]
            fn test_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.overflow = false;

                let wait_cycles = cpu.execute_instruction(0x70);

                assert_eq!(cpu.registers.pc, 0x0201);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.overflow = true;

                let wait_cycles = cpu.execute_instruction(0x70);

                assert_eq!(cpu.registers.pc, 0x0205);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x02FE;
                cpu.internal_ram[0xFE] = 0x04;
                cpu.registers.p.overflow = true;

                let wait_cycles = cpu.execute_instruction(0x70);

                assert_eq!(cpu.registers.pc, 0x0303);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_negative_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-4 as i8) as u8;
                cpu.registers.p.overflow = true;

                let wait_cycles = cpu.execute_instruction(0x70);

                assert_eq!(cpu.registers.pc, 0x0200);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-5 as i8) as u8;
                cpu.registers.p.overflow = true;

                let wait_cycles = cpu.execute_instruction(0x70);

                assert_eq!(cpu.registers.pc, 0x01FF);
                assert_eq!(wait_cycles, 4);
            }
        }

        mod bvc
        {
            use super::*;

            #[test]
            fn test_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.overflow = true;

                let wait_cycles = cpu.execute_instruction(0x50);

                assert_eq!(cpu.registers.pc, 0x0201);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.internal_ram[0x00] = 0x04;
                cpu.registers.p.overflow = false;

                let wait_cycles = cpu.execute_instruction(0x50);

                assert_eq!(cpu.registers.pc, 0x0205);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x02FE;
                cpu.internal_ram[0xFE] = 0x04;
                cpu.registers.p.overflow = false;

                let wait_cycles = cpu.execute_instruction(0x50);

                assert_eq!(cpu.registers.pc, 0x0303);
                assert_eq!(wait_cycles, 4);
            }

            #[test]
            fn test_negative_relative()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-4 as i8) as u8;
                cpu.registers.p.overflow = false;

                let wait_cycles = cpu.execute_instruction(0x50);

                assert_eq!(cpu.registers.pc, 0x0200);
                assert_eq!(wait_cycles, 3);

                cpu.registers.pc = 0x0203;
                cpu.internal_ram[0x03] = (-5 as i8) as u8;
                cpu.registers.p.overflow = false;

                let wait_cycles = cpu.execute_instruction(0x50);

                assert_eq!(cpu.registers.pc, 0x01FF);
                assert_eq!(wait_cycles, 4);
            }
        }

        mod clc
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x18);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x18);

                assert_eq!(cpu.registers.p.carry, false);
                assert_eq!(wait_cycles, 2);
            }
        }

        mod cld
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.decimal = false;

                let wait_cycles = cpu.execute_instruction(0xD8);

                assert_eq!(cpu.registers.p.decimal, false);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.p.decimal = true;

                let wait_cycles = cpu.execute_instruction(0xD8);

                assert_eq!(cpu.registers.p.decimal, false);
                assert_eq!(wait_cycles, 2);
            }
        }

        mod cli
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.interrupt_disable = false;

                let wait_cycles = cpu.execute_instruction(0x58);

                assert_eq!(cpu.registers.p.interrupt_disable, false);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.p.interrupt_disable = true;

                let wait_cycles = cpu.execute_instruction(0x58);

                assert_eq!(cpu.registers.p.interrupt_disable, false);
                assert_eq!(wait_cycles, 2);
            }
        }

        mod clv
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.overflow = false;

                let wait_cycles = cpu.execute_instruction(0xB8);

                assert_eq!(cpu.registers.p.overflow, false);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.p.overflow = true;

                let wait_cycles = cpu.execute_instruction(0xB8);

                assert_eq!(cpu.registers.p.overflow, false);
                assert_eq!(wait_cycles, 2);
            }
        }

        mod sec
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = false;

                let wait_cycles = cpu.execute_instruction(0x38);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x38);

                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(wait_cycles, 2);
            }
        }

        mod sed
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.decimal = false;

                let wait_cycles = cpu.execute_instruction(0xF8);

                assert_eq!(cpu.registers.p.decimal, true);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0xF8);

                assert_eq!(cpu.registers.p.decimal, true);
                assert_eq!(wait_cycles, 2);
            }
        }

        mod sei
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.p.interrupt_disable = false;

                let wait_cycles = cpu.execute_instruction(0x78);

                assert_eq!(cpu.registers.p.interrupt_disable, true);
                assert_eq!(wait_cycles, 2);

                cpu.registers.pc = 0x0200;
                cpu.registers.p.carry = true;

                let wait_cycles = cpu.execute_instruction(0x78);

                assert_eq!(cpu.registers.p.interrupt_disable, true);
                assert_eq!(wait_cycles, 2);
            }
        }

        mod brk
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0201;
                cpu.registers.p.carry = true;
                cpu.registers.p.interrupt_disable = false;
                cpu.registers.stack_pointer = 0xFD;

                let wait_cycles = cpu.execute_instruction(0x00);

                // dummy mapper returns address 0x8000 when loading irq/brk vector
                assert_eq!(cpu.registers.pc, 0x8000);
                assert_eq!(cpu.registers.stack_pointer, 0xFA);
                assert_eq!(cpu.stack[0xFD], 0x02);
                assert_eq!(cpu.stack[0xFC], 0x01);
                assert_eq!(cpu.stack[0xFB], 0b0011_0001);
                assert_eq!(cpu.registers.p.interrupt_disable, true);
                assert_eq!(wait_cycles, 7);
            }
        }

        mod rti
        {
            use super::*;

            #[test]
            fn test_implicit()
            {
                let mut cpu = Cpu::new_dummy();
                cpu.registers.pc = 0x0200;
                cpu.registers.stack_pointer = 0xFA;
                cpu.stack[0xFD] = 0x04;
                cpu.stack[0xFC] = 0x40;
                cpu.stack[0xFB] = 0b0011_0001;
                cpu.registers.p.carry = false;
                cpu.registers.p.interrupt_disable = true;
                cpu.registers.p.negative = true;

                let wait_cycles = cpu.execute_instruction(0x40);

                assert_eq!(cpu.registers.pc, 0x0440);
                assert_eq!(cpu.registers.p.carry, true);
                assert_eq!(cpu.registers.p.zero, false);
                assert_eq!(cpu.registers.p.interrupt_disable, false);
                assert_eq!(cpu.registers.p.decimal, false);
                assert_eq!(cpu.registers.p.overflow, false);
                assert_eq!(cpu.registers.p.negative, false);
                assert_eq!(cpu.registers.stack_pointer, 0xFD);
                assert_eq!(wait_cycles, 6);
            }
        }
    }
}