use super::Cpu;

pub trait AddressingMode
{
    fn read(&self, cpu: &Cpu) -> u8;
    fn write(&self, cpu: &mut Cpu, data: u8);
    fn address(&self) -> u16;
    fn page_boundary_crossed(&self) -> bool;
}

pub struct Implicit;
impl AddressingMode for Implicit
{
    fn read(&self, _cpu: &Cpu) -> u8 { 0 }
    fn write(&self, _cpu: &mut Cpu, _data: u8) { }
    fn address(&self) -> u16 { 0 }
    fn page_boundary_crossed(&self) -> bool { false }
}

pub struct Accumulator;
impl AddressingMode for Accumulator
{
    fn read(&self, cpu: &Cpu) -> u8 { cpu.registers.a }
    fn write(&self, cpu: &mut Cpu, data: u8) { cpu.registers.a = data }
    fn address(&self) -> u16 { 0 }
    fn page_boundary_crossed(&self) -> bool { false }
}

pub struct Immediate
{
    value: u8,
}
impl Immediate
{
    pub fn new(cpu: &mut Cpu) -> Immediate
    {
        Immediate {value: cpu.fetch()}
    }
}
impl AddressingMode for Immediate
{
    fn read(&self, _cpu: &Cpu) -> u8 { self.value }
    fn write(&self, _cpu: &mut Cpu, _data: u8) { }
    fn address(&self) -> u16 { 0 }
    fn page_boundary_crossed(&self) -> bool { false }
}

pub struct Relative
{
    offset: u8,
}
impl Relative
{
    pub fn new(cpu: &mut Cpu) -> Relative
    {
        Relative {offset: cpu.fetch()}
    }
}
impl AddressingMode for Relative
{
    fn read(&self, _cpu: &Cpu) -> u8 { self.offset }
    fn write(&self, _cpu: &mut Cpu, _data: u8) { }
    fn address(&self) -> u16 { 0 }
    fn page_boundary_crossed(&self) -> bool { false }
}

pub struct MemoryAccess
{
    address: u16,
    page_boundary_crossed: bool,
}
impl MemoryAccess
{
    // Zero Page
    pub fn new_zero_page(cpu: &mut Cpu) -> MemoryAccess
    {
        MemoryAccess {address: cpu.fetch() as u16, page_boundary_crossed: false}
    }

    pub fn new_indexed_zero_page(cpu: &mut Cpu, index: u8) -> MemoryAccess { MemoryAccess {address: cpu.fetch().wrapping_add(index) as u16, page_boundary_crossed: false} }

    // Absolute
    pub fn new_absolute(cpu: &mut Cpu) -> MemoryAccess
    {
        MemoryAccess {address: cpu.fetch() as u16 | (cpu.fetch() as u16) << 8, page_boundary_crossed: false}
    }

    pub fn new_indexed_absolute(cpu: &mut Cpu, index: u8) -> MemoryAccess
    {
        let address = cpu.fetch() as u16 | (cpu.fetch() as u16) << 8;
        // page boundaries check;
        let page_boundary_crossed = address.wrapping_add(index as u16) & 0xFF00 != address & 0xFF00;
        MemoryAccess {address: address.wrapping_add(index as u16), page_boundary_crossed}
    }

    // Indirect
    pub fn new_indirect(cpu: &mut Cpu) -> MemoryAccess
    {
        let indirect_address = cpu.fetch() as u16 | (cpu.fetch() as u16) << 8;
        let address_lsb = cpu.load(indirect_address) as u16;
        let address_msb = (cpu.load(indirect_address + 1) as u16) << 8;
        MemoryAccess {address: address_lsb | address_msb, page_boundary_crossed: false}
    }

    pub fn new_indexed_indirect(cpu: &mut Cpu, index: u8) -> MemoryAccess
    {
        let indirect_address: u8 = cpu.fetch().wrapping_add(index);
        let address_lsb = cpu.load(indirect_address as u16) as u16;
        let address_msb = (cpu.load(indirect_address.wrapping_add(1)as u16) as u16) << 8;
        MemoryAccess {address: address_lsb | address_msb, page_boundary_crossed: false}
    }

    pub fn new_indirect_indexed(cpu: &mut Cpu, index: u8) -> MemoryAccess
    {
        let indirect_address: u8 = cpu.fetch();
        let address_lsb = cpu.load(indirect_address as u16) as u16;
        let address_msb = (cpu.load(indirect_address.wrapping_add(1) as u16) as u16) << 8;
        // page boundaries check;
        let page_boundary_crossed = (address_lsb | address_msb).wrapping_add(index as u16) & 0xFF00 != (address_lsb | address_msb) & 0xFF00;
        MemoryAccess {address: (address_lsb | address_msb).wrapping_add(index as u16), page_boundary_crossed}
    }
}
impl AddressingMode for MemoryAccess
{
    fn read(&self, cpu: &Cpu) -> u8 { cpu.load(self.address) }
    fn write(&self, cpu: &mut Cpu, data: u8) { cpu.write(self.address, data); }
    fn address(&self) -> u16 { self.address }
    fn page_boundary_crossed(&self) -> bool { self.page_boundary_crossed }
}
