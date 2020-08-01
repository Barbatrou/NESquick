use super::Cpu;
use super::InstructionResult;
use super::AddressingMode;
use std::sync::atomic::Ordering::AcqRel;

enum LoadDestination
{
    Accumulator,
    X,
    Y,
}

impl Cpu
{
    // Load/store
    fn load_instruction(&mut self, data: u8, dest: LoadDestination)
    {
        self.registers.set_status_zero(data == 0);
        self.registers.set_status_negative(data & 0x80 == 0x80);
        match dest {
            LoadDestination::Accumulator => self.registers.a = data,
            LoadDestination::X => self.registers.x = data,
            LoadDestination::Y => self.registers.y = data,
        }
    }

    pub fn lda(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.load_instruction(addressing_mode.read(&self), LoadDestination::Accumulator);
        InstructionResult::Ok
    }

    pub fn ldx(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.load_instruction(addressing_mode.read(&self), LoadDestination::X);
        InstructionResult::Ok
    }

    pub fn ldy(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.load_instruction(addressing_mode.read(&self), LoadDestination::Y);
        InstructionResult::Ok
    }

    // Arithmetic
    pub fn adc(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let val = addressing_mode.read(&self);
        let result = self.registers.a as u16 + val as u16 + self.registers.p.carry as u16;
        self.registers.set_status_carry(result > 0xFF);
        self.registers.set_status_zero(result as u8 == 0);
        self.registers.set_status_overflow((self.registers.a ^ result as u8) & (val ^ result as u8) & 0x80 == 0x80);
        self.registers.set_status_negative(result as u8 & 0x80 == 0x80);
        self.registers.a = result as u8;
        InstructionResult::Ok
    }

    // System functions
    pub fn brk(&self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        InstructionResult::Ok
    }
}