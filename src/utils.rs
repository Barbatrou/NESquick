
// trait for clocked device such as CPU and PPU
pub trait Clocked {
    // function to execute a clock step of the device
    fn  clock(&mut self);
}
