use std::fs;

pub trait Mapper
{
    fn read(&self, address: u16) -> u8;
    fn write(&mut self, address: u16, data: u8);
}

pub fn load_cartridge(filepath: &str) -> Box<dyn Mapper>
{
    let rom_content = fs::read(filepath).expect(&format!("Could not load rom {}", filepath));
    if !(rom_content[0] ==  0x4E &&
        rom_content[1] == 0x45 &&
        rom_content [2] == 0x53 &&
        rom_content[3] ==  0x1A) {
        panic!(format!("file {} isn't an INES file", filepath));
    }
    match (rom_content[6] & 0xF0 >> 4) | (rom_content[7] & 0xF0) {
        0 => Box::new(NROM::new(rom_content)),
        _ => Box::new(DummyMapper::new()),
    }
}

pub struct DummyMapper {}
impl DummyMapper
{
    pub fn new() -> DummyMapper { DummyMapper{} }
}
impl Mapper for DummyMapper
{
    fn read(&self, _address: u16) -> u8 { 0 }
    fn write(&mut self, _address: u16, _data: u8) {}
}

pub struct NROM
{
    rom: [u8; 0x8000],
    ram: [u8; 0x2000],
    rom_size: usize,
}
impl NROM
{
    pub fn new(rom_content: Vec<u8>) -> NROM
    {
        let rom_size = rom_content[4] as usize * 0x4000;
        let mut rom = [0; 0x8000];
        let rom_start = if (rom_content[6] & 0b0000_00100) >> 2 == 1 {528} else {16};
        rom[..rom_size].copy_from_slice(&rom_content[rom_start..(rom_start + rom_size)]);
        NROM{
            rom,
            ram: [0; 0x2000],
            rom_size,
        }
    }
}
impl Mapper for NROM
{
    fn read(&self, address: u16) -> u8
    {
        if address < 0x6000 {
            return 0
        }
        let address = address - 0x6000;
        match address {
            0x0000..=0x1FFF => self.ram[address as usize],
            0x2000..=0x9FFF => self.rom[(address - 0x2000) as usize % self.rom_size],
            _ => 0
        }
    }

    fn write(&mut self, address: u16, data: u8)
    {
        if address >= 0x6000 {
            let address = address - 0x6000;
            match address {
                0x00..=0x1FFF => self.ram[address as usize] = data,
                0x2000..=0x9FFF => self.rom[(address as usize - 0x2000) % self.rom_size] = data,
                _ => {}
            }
        }
    }
}