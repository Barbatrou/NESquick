mod utils;
mod cpu;

use cpu::{
    Cpu,
    load_cartridge,
};
use crate::utils::Clocked;

fn main()
{
    let cartridge = load_cartridge("rom_tests/nestest/nestest.nes");
    let mut cpu = Cpu::new(cartridge);
    cpu.set_pc(0xC000);


    while cpu.cycles < 26554 {
        cpu.clock();
    }
}
