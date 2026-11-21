use std::num::NonZeroU32;

use crate::domain::cpu::VcpuCount;
use crate::domain::error::ShapeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shape {
    vcpu: VcpuCount,
    memory_gib: NonZeroU32,
}

impl Shape {
    pub const MAX_VCPU: u16 = 256;
    pub const MAX_MEMORY_GIB: u32 = 4096;

    pub const fn new(vcpu: u16, memory_gib: u32) -> Result<Self, ShapeError> {
        if vcpu == 0 {
            return Err(ShapeError::ZeroVcpu);
        }
        if vcpu > Self::MAX_VCPU {
            return Err(ShapeError::VcpuTooLarge { max: Self::MAX_VCPU });
        }
        if memory_gib == 0 {
            return Err(ShapeError::ZeroMemory);
        }
        if memory_gib > Self::MAX_MEMORY_GIB {
            return Err(ShapeError::MemoryTooLarge {
                max: Self::MAX_MEMORY_GIB,
            });
        }
        let cores = match VcpuCount::new(vcpu) {
            Ok(value) => value,
            Err(_) => return Err(ShapeError::ZeroVcpu),
        };
        let memory = match NonZeroU32::new(memory_gib) {
            Some(value) => value,
            None => return Err(ShapeError::ZeroMemory),
        };
        Ok(Self {
            vcpu: cores,
            memory_gib: memory,
        })
    }

    pub const fn vcpu(&self) -> VcpuCount {
        self.vcpu
    }

    pub const fn memory_gib(&self) -> u32 {
        self.memory_gib.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_shape_holds_independent_cpu_and_memory() {
        let shape = Shape::new(8, 64).expect("valid shape");
        assert_eq!(shape.vcpu().get(), 8);
        assert_eq!(shape.memory_gib(), 64);
        assert_ne!(shape, Shape::new(8, 8).expect("valid shape"));
    }

    #[test]
    fn domain_shape_rejects_zero_and_out_of_bounds_resources() {
        assert_eq!(Shape::new(0, 8), Err(ShapeError::ZeroVcpu));
        assert_eq!(Shape::new(4, 0), Err(ShapeError::ZeroMemory));
        assert_eq!(
            Shape::new(Shape::MAX_VCPU + 1, 8),
            Err(ShapeError::VcpuTooLarge { max: 256 })
        );
        assert_eq!(
            Shape::new(4, Shape::MAX_MEMORY_GIB + 1),
            Err(ShapeError::MemoryTooLarge { max: 4096 })
        );
    }
}
