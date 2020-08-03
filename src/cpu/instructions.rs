use super::Cpu;
use super::InstructionResult;
use super::AddressingMode;
use super::Interrupts;


enum LoadStoreLocation
{
    Accumulator,
    X,
    Y,
}

impl Cpu
{
    // Load/store
    fn load_instruction(&mut self, data: u8, dest: LoadStoreLocation)
    {
        self.registers.set_status_zero(data == 0);
        self.registers.set_status_negative(data & 0x80 == 0x80);
        match dest {
            LoadStoreLocation::Accumulator => self.registers.a = data,
            LoadStoreLocation::X => self.registers.x = data,
            LoadStoreLocation::Y => self.registers.y = data,
        }
    }

    pub fn lda(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.load_instruction(addressing_mode.read(&self), LoadStoreLocation::Accumulator);
        InstructionResult::Ok
    }

    pub fn ldx(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.load_instruction(addressing_mode.read(&self), LoadStoreLocation::X);
        InstructionResult::Ok
    }

    pub fn ldy(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.load_instruction(addressing_mode.read(self), LoadStoreLocation::Y);
        InstructionResult::Ok
    }

    fn store_instruction(&self, src: LoadStoreLocation) -> u8
    {
        match src {
            LoadStoreLocation::Accumulator => self.registers.a,
            LoadStoreLocation::X => self.registers.x,
            LoadStoreLocation::Y => self.registers.y,
        }
    }

    pub fn sta(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        addressing_mode.write(self, self.store_instruction(LoadStoreLocation::Accumulator));
        InstructionResult::Ok
    }

    pub fn stx(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        addressing_mode.write(self, self.store_instruction(LoadStoreLocation::X));
        InstructionResult::Ok
    }

    pub fn sty(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        addressing_mode.write(self, self.store_instruction(LoadStoreLocation::Y));
        InstructionResult::Ok
    }

    // Register transfers
    pub fn tax(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.set_status_zero(self.registers.a == 0);
        self.registers.set_status_negative(self.registers.a & 0x80 == 0x80);
        self.registers.x = self.registers.a;
        InstructionResult::Ok
    }

    pub fn tay(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.set_status_zero(self.registers.a == 0);
        self.registers.set_status_negative(self.registers.a & 0x80 == 0x80);
        self.registers.y = self.registers.a;
        InstructionResult::Ok
    }

    pub fn txa(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.set_status_zero(self.registers.x == 0);
        self.registers.set_status_negative(self.registers.x & 0x80 == 0x80);
        self.registers.a = self.registers.x;
        InstructionResult::Ok
    }

    pub fn tya(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.set_status_zero(self.registers.y == 0);
        self.registers.set_status_negative(self.registers.y & 0x80 == 0x80);
        self.registers.a = self.registers.y;
        InstructionResult::Ok
    }

    // Stack operation
    pub fn tsx(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.set_status_zero(self.registers.stack_pointer == 0);
        self.registers.set_status_negative(self.registers.stack_pointer & 0x80 == 0x80);
        self.registers.x = self.registers.stack_pointer;
        InstructionResult::Ok
    }

    pub fn txs(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.stack_pointer = self.registers.x;
        InstructionResult::Ok
    }

    pub fn pha(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.push(self.registers.a);
        InstructionResult::Ok
    }

    pub fn php(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.push(self.registers.p.get_byte() | 0b0011_0000);
        InstructionResult::Ok
    }

    pub fn pla(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.a = self.pop();
        self.registers.set_status_zero(self.registers.a == 0);
        self.registers.set_status_negative(self.registers.a & 0x80 == 0x80);
        InstructionResult::Ok
    }

    pub fn plp(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let data = self.pop() & 0b1100_1111;
        self.registers.p.set_byte(data);
        InstructionResult::Ok
    }

    // Logical

    pub fn and(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.a = self.registers.a & addressing_mode.read(self);
        self.registers.set_status_zero(self.registers.a == 0);
        self.registers.set_status_negative(self.registers.a & 0x80 == 0x80);
        InstructionResult::Ok
    }

    pub fn ora(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.a = self.registers.a | addressing_mode.read(self);
        self.registers.set_status_zero(self.registers.a == 0);
        self.registers.set_status_negative(self.registers.a & 0x80 == 0x80);
        InstructionResult::Ok
    }

    pub fn eor(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.a = self.registers.a ^ addressing_mode.read(self);
        self.registers.set_status_zero(self.registers.a == 0);
        self.registers.set_status_negative(self.registers.a & 0x80 == 0x80);
        InstructionResult::Ok
    }

    pub fn bit(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let data = addressing_mode.read(self);
        self.registers.set_status_zero(self.registers.a & data == 0);
        self.registers.set_status_negative(data & 0b1000_0000 == 0x80);
        self.registers.set_status_overflow(data & 0b0100_0000 == 0x40);
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

    pub fn sbc(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let val = addressing_mode.read(&self);
        let result = (self.registers.a as u16).wrapping_sub(val as u16).wrapping_sub(!self.registers.p.carry as u16);
        self.registers.set_status_carry(result <= 0xFF);
        self.registers.set_status_zero(result as u8 == 0);
        self.registers.set_status_overflow((self.registers.a ^ result as u8) & (val ^ result as u8) & 0x80 == 0x80);
        self.registers.set_status_negative(result as u8 & 0x80 == 0x80);
        self.registers.a = result as u8;
        InstructionResult::Ok
    }

    pub fn cmp(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let result: i16 = self.registers.a as i16 - addressing_mode.read(self) as i16;
        self.registers.set_status_carry(result >= 0);
        self.registers.set_status_zero(result == 0);
        self.registers.set_status_negative(result & 0x80 == 0x80);
        InstructionResult::Ok
    }

    pub fn cpx(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let result: i16 = self.registers.x as i16 - addressing_mode.read(self) as i16;
        self.registers.set_status_carry(result >= 0);
        self.registers.set_status_zero(result == 0);
        self.registers.set_status_negative(result & 0x80 == 0x80);
        InstructionResult::Ok
    }

    pub fn cpy(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let result: i16 = self.registers.y as i16 - addressing_mode.read(self) as i16;
        self.registers.set_status_carry(result >= 0);
        self.registers.set_status_zero(result == 0);
        self.registers.set_status_negative(result & 0x80 == 0x80);
        InstructionResult::Ok
    }

    // Increments and Decrements
    pub fn inc(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let data = addressing_mode.read(self);
        self.registers.set_status_zero(data.wrapping_add(1) == 0);
        self.registers.set_status_negative(data.wrapping_add(1) & 0x80 == 0x80);
        addressing_mode.write(self, data.wrapping_add(1));
        InstructionResult::Ok
    }

    pub fn inx(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.x = self.registers.x.wrapping_add(1);
        self.registers.set_status_zero(self.registers.x == 0);
        self.registers.set_status_negative(self.registers.x & 0x80 == 0x80);
        InstructionResult::Ok
    }

    pub fn iny(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.y = self.registers.y.wrapping_add(1);
        self.registers.set_status_zero(self.registers.y == 0);
        self.registers.set_status_negative(self.registers.y & 0x80 == 0x80);
        InstructionResult::Ok
    }

    pub fn dec(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let data = addressing_mode.read(self);
        self.registers.set_status_zero(data.wrapping_sub(1) == 0);
        self.registers.set_status_negative(data.wrapping_sub(1) & 0x80 == 0x80);
        addressing_mode.write(self, data.wrapping_sub(1));
        InstructionResult::Ok
    }

    pub fn dex(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.x = self.registers.x.wrapping_sub(1);
        self.registers.set_status_zero(self.registers.x == 0);
        self.registers.set_status_negative(self.registers.x & 0x80 == 0x80);
        InstructionResult::Ok
    }

    pub fn dey(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.y = self.registers.y.wrapping_sub(1);
        self.registers.set_status_zero(self.registers.y == 0);
        self.registers.set_status_negative(self.registers.y & 0x80 == 0x80);
        InstructionResult::Ok
    }

    // Shifts
    pub fn asl(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let data = addressing_mode.read(self);
        self.registers.set_status_carry(data & 0x80 == 0x80);
        let result = data << 1;
        self.registers.set_status_zero(result == 0);
        self.registers.set_status_negative(result & 0x80 == 0x80);
        addressing_mode.write(self, result);
        InstructionResult::Ok
    }

    pub fn lsr(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let data = addressing_mode.read(self);
        self.registers.set_status_carry(data & 0x01 == 0x01);
        let result = data >> 1;
        self.registers.set_status_zero(result == 0);
        self.registers.set_status_negative(result & 0x80 == 0x80);
        addressing_mode.write(self, result);
        InstructionResult::Ok
    }

    pub fn rol(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let data = addressing_mode.read(self);
        let old_carry = (self.registers.p.carry as u8);
        self.registers.set_status_carry(data & 0x80 == 0x80);
        let result = (data << 1) | old_carry;
        self.registers.set_status_zero(result == 0);
        self.registers.set_status_negative(result & 0x80 == 0x80);
        addressing_mode.write(self, result);
        InstructionResult::Ok
    }

    pub fn ror(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let data = addressing_mode.read(self);
        let old_carry = (self.registers.p.carry as u8) << 7;
        self.registers.set_status_carry(data & 0x01 == 0x01);
        let result = (data >> 1) | old_carry;
        self.registers.set_status_zero(result == 0);
        self.registers.set_status_negative(result & 0x80 == 0x80);
        addressing_mode.write(self, result);
        InstructionResult::Ok
    }

    // Jumps and calls
    pub fn jmp(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.pc = addressing_mode.address();
        InstructionResult::Ok
    }

    pub fn jsr(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let address = self.registers.pc.wrapping_sub(1);
        self.push((address >> 8) as u8);
        self.push(address as u8);
        self.registers.pc = addressing_mode.address();
        InstructionResult::Ok
    }

    pub fn rts(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let address: u16 =  self.pop() as u16 | ((self.pop() as u16) << 8);
        let address = address.wrapping_add(1);
        self.registers.pc = address;
        InstructionResult::Ok
    }

    // Branch
    pub fn bcc(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        if !self.registers.p.carry {
            let old_pc = self.registers.pc;
            let offset: i8 = addressing_mode.read(self) as i8;
            self.registers.pc = (self.registers.pc as i32).wrapping_add(offset as i32) as u16;
            InstructionResult::Branch(if old_pc & 0xFF00 == self.registers.pc & 0xFF00 {1} else {2})
        } else {
            InstructionResult::NOP
        }
    }

    pub fn bcs(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        if self.registers.p.carry {
            let old_pc = self.registers.pc;
            let offset: i8 = addressing_mode.read(self) as i8;
            self.registers.pc = (self.registers.pc as i32).wrapping_add(offset as i32) as u16;
            InstructionResult::Branch(if old_pc & 0xFF00 == self.registers.pc & 0xFF00 {1} else {2})
        } else {
            InstructionResult::NOP
        }
    }

    pub fn beq(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        if self.registers.p.zero {
            let old_pc = self.registers.pc;
            let offset: i8 = addressing_mode.read(self) as i8;
            self.registers.pc = (self.registers.pc as i32).wrapping_add(offset as i32) as u16;
            InstructionResult::Branch(if old_pc & 0xFF00 == self.registers.pc & 0xFF00 {1} else {2})
        } else {
            InstructionResult::NOP
        }
    }

    pub fn bmi(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        if self.registers.p.negative {
            let old_pc = self.registers.pc;
            let offset: i8 = addressing_mode.read(self) as i8;
            self.registers.pc = (self.registers.pc as i32).wrapping_add(offset as i32) as u16;
            InstructionResult::Branch(if old_pc & 0xFF00 == self.registers.pc & 0xFF00 {1} else {2})
        } else {
            InstructionResult::NOP
        }
    }

    pub fn bne(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        if !self.registers.p.zero {
            let old_pc = self.registers.pc;
            let offset: i8 = addressing_mode.read(self) as i8;
            self.registers.pc = (self.registers.pc as i32).wrapping_add(offset as i32) as u16;
            InstructionResult::Branch(if old_pc & 0xFF00 == self.registers.pc & 0xFF00 {1} else {2})
        } else {
            InstructionResult::NOP
        }
    }

    pub fn bpl(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        if !self.registers.p.negative {
            let old_pc = self.registers.pc;
            let offset: i8 = addressing_mode.read(self) as i8;
            self.registers.pc = (self.registers.pc as i32).wrapping_add(offset as i32) as u16;
            InstructionResult::Branch(if old_pc & 0xFF00 == self.registers.pc & 0xFF00 {1} else {2})
        } else {
            InstructionResult::NOP
        }
    }

    pub fn bvc(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        if !self.registers.p.overflow {
            let old_pc = self.registers.pc;
            let offset: i8 = addressing_mode.read(self) as i8;
            self.registers.pc = (self.registers.pc as i32).wrapping_add(offset as i32) as u16;
            InstructionResult::Branch(if old_pc & 0xFF00 == self.registers.pc & 0xFF00 {1} else {2})
        } else {
            InstructionResult::NOP
        }
    }

    pub fn bvs(&mut self, addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        if self.registers.p.overflow {
            let old_pc = self.registers.pc;
            let offset: i8 = addressing_mode.read(self) as i8;
            self.registers.pc = (self.registers.pc as i32).wrapping_add(offset as i32) as u16;
            InstructionResult::Branch(if old_pc & 0xFF00 == self.registers.pc & 0xFF00 {1} else {2})
        } else {
            InstructionResult::NOP
        }
    }

    // Status flags change
    pub fn clc(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.set_status_carry(false);
        InstructionResult::Ok
    }

    pub fn cld(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.set_status_decimal(false);
        InstructionResult::Ok
    }

    pub fn cli(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.set_status_interupt_disable(false);
        InstructionResult::Ok
    }

    pub fn clv(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.set_status_overflow(false);
        InstructionResult::Ok
    }

    pub fn sec(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.set_status_carry(true);
        InstructionResult::Ok
    }

    pub fn sed(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.set_status_decimal(true);
        InstructionResult::Ok
    }

    pub fn sei(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.registers.set_status_interupt_disable(true);
        InstructionResult::Ok
    }

    // System functions
    pub fn brk(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        self.interrupt(Interrupts::Break);
        InstructionResult::Ok
    }

    pub fn rti(&mut self, _addressing_mode: &dyn AddressingMode) -> InstructionResult
    {
        let status = self.pop();
        self.registers.p.set_byte(status);
        self.registers.pc = self.pop() as u16 | ((self.pop() as u16) << 8);
        InstructionResult::Ok
    }
}