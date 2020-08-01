use super::Cpu;

pub trait AddressSpace
{
    fn read(&self, cpu: &Cpu) -> u8;
    fn write(&self, cpu: &mut Cpu, data: u8);
}


pub struct ZeroPageAddressSpace
{
    index: usize,
}
impl ZeroPageAddressSpace
{
    pub fn new(address: u8) -> ZeroPageAddressSpace { ZeroPageAddressSpace{index: address as usize} }
}
impl AddressSpace for ZeroPageAddressSpace
{
    fn read(&self, cpu: &Cpu) -> u8 { cpu.zero_page_ram[self.index] }
    fn write(&self, cpu: &mut Cpu, data: u8) { cpu.zero_page_ram[self.index] = data; }
}


pub struct StackAddressSpace
{
    index: usize,
}
impl StackAddressSpace
{
    pub fn new(address: u8) -> StackAddressSpace { StackAddressSpace{index: address as usize} }
}
impl AddressSpace for StackAddressSpace
{
    fn read(&self, cpu: &Cpu) -> u8 { cpu.stack[self.index] }
    fn write(&self, cpu: &mut Cpu, data: u8) { cpu.stack[self.index] = data }
}


pub struct RamAddressSpace
{
    address: usize,
}
impl RamAddressSpace
{
    pub fn new(address: u16) -> RamAddressSpace { RamAddressSpace{address: address as usize} }
}
impl AddressSpace for RamAddressSpace
{
    fn read(&self, cpu: &Cpu) -> u8 { cpu.internal_ram[self.address] }
    fn write(&self, cpu: &mut Cpu, data: u8) { cpu.internal_ram[self.address] = data; }
}


pub struct PpuRegistersAddressSpace
{
}
impl PpuRegistersAddressSpace
{
    pub fn new(_address: u16) -> PpuRegistersAddressSpace { PpuRegistersAddressSpace{} }
}
impl AddressSpace for PpuRegistersAddressSpace
{
    fn read(&self, _cpu: &Cpu) -> u8 { 0 }
    fn write(&self, _cpu: &mut Cpu, _data: u8) { }
}


pub struct ApuRegistersAddressSpace
{
}
impl ApuRegistersAddressSpace
{
    pub fn new(_address: u16) -> ApuRegistersAddressSpace { ApuRegistersAddressSpace{} }
}
impl AddressSpace for ApuRegistersAddressSpace
{
    fn read(&self, _cpu: &Cpu) -> u8 { 0 }
    fn write(&self, _cpu: &mut Cpu, _data: u8) { }
}


pub struct IORegistersAddressSpace
{
}
impl IORegistersAddressSpace
{
    pub fn new(_address: u16) -> IORegistersAddressSpace { IORegistersAddressSpace{} }
}
impl AddressSpace for IORegistersAddressSpace
{
    fn read(&self, _cpu: &Cpu) -> u8 { 0 }
    fn write(&self, _cpu: &mut Cpu, _data: u8) { }
}

pub struct CartridgeAddressSpace
{
    address: u16,
}
impl CartridgeAddressSpace
{
    pub fn new(address: u16) -> CartridgeAddressSpace { CartridgeAddressSpace{address} }
}
impl AddressSpace for CartridgeAddressSpace
{
    fn read(&self, cpu: &Cpu) -> u8 { cpu.cartridge.read(self.address) }
    fn write(&self, cpu: &mut Cpu, data: u8) { cpu.cartridge.write(self.address, data) }
}

pub struct NullAddressSpace {}
impl NullAddressSpace
{
    pub fn new() -> NullAddressSpace { NullAddressSpace{} }
}
impl AddressSpace for NullAddressSpace
{
    fn read(&self, _cpu: &Cpu) -> u8 { 0 }
    fn write(&self, _cpu: &mut Cpu, _data: u8) { }
}









