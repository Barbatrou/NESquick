
pub struct Status
{
    pub carry: bool,
    pub zero: bool,
    pub interrupt_disable: bool,
    pub decimal: bool,
    pub overflow: bool,
    pub negative: bool,
}

impl Status
{
    pub fn get_byte(&self) -> u8
    {
        (self.carry as u8)
            | (self.zero as u8) << 1
            | (self.interrupt_disable as u8) << 2
            | (self.decimal as u8) << 3
            | (self.overflow as u8) << 6
            | (self.negative as u8) << 7
    }

    pub fn set_byte(&mut self, status: u8)
    {
        self.carry = (status & 0b0000_0001) == 1;
        self.zero = (status & 0b0000_0010) >> 1 == 1;
        self.interrupt_disable = (status & 0b0000_0100) >> 2 == 1;
        self.decimal = (status & 0b0000_1000) >> 3 == 1;
        self.overflow = (status & 0b0100_0000) >> 6 == 1;
        self.negative = (status & 0b1000_0000) >> 7 == 1;
    }
}

pub struct Registers
{
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub p: Status,
    pub pc: u16,
    pub stack_pointer: u8,
}

impl Registers
{
    pub fn new() -> Registers
    {
        Registers {
            a: 0,
            x: 0,
            y: 0,
            p: Status {
                carry: false,
                zero: false,
                interrupt_disable: true,
                decimal: false,
                overflow: false,
                negative: false,
            },
            pc: 0x8000,
            stack_pointer: 0xFD,
        }
    }

    pub fn set_status_carry(&mut self, status: bool) -> &mut Self { self.p.carry = status; self }
    pub fn set_status_zero(&mut self, status: bool) -> &mut Self { self.p.zero = status; self }
    pub fn set_status_interupt_disable(&mut self, status: bool) -> &mut Self { self.p.interrupt_disable = status; self }
    pub fn set_status_decimal(&mut self, status: bool) -> &mut Self { self.p.decimal = status; self }
    pub fn set_status_overflow(&mut self, status: bool) -> &mut Self { self.p.overflow = status; self }
    pub fn set_status_negative(&mut self, status: bool) -> &mut Self { self.p.negative = status; self }

    pub fn flip_status_carry(&mut self) -> &mut Self { self.p.carry = !self.p.carry; self }
    pub fn flip_status_zero(&mut self) -> &mut Self { self.p.zero = !self.p.zero; self }
    pub fn flip_status_interupt_disable(&mut self) -> &mut Self { self.p.interrupt_disable = !self.p.interrupt_disable; self }
    pub fn flip_status_decimal(&mut self) -> &mut Self { self.p.decimal = !self.p.decimal; self }
    pub fn flip_status_overflow(&mut self) -> &mut Self { self.p.overflow = !self.p.overflow; self }
    pub fn flip_status_negative(&mut self) -> &mut Self { self.p.negative = !self.p.negative; self }
}
