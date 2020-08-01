use super::Cpu;
use super::InstructionResult;
use super::AddressingMode;

impl Cpu
{
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

    pub fn brk(&self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        InstructionResult::Ok
    }
}