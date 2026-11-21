use std::num::NonZeroU16;

use crate::domain::error::CpuError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThreadsPerCore(NonZeroU16);

impl ThreadsPerCore {
    pub const MAX: u16 = 16;

    pub const ONE: Self = Self(match NonZeroU16::new(1) {
        Some(value) => value,
        None => panic!("1 is nonzero"),
    });

    pub const TWO: Self = Self(match NonZeroU16::new(2) {
        Some(value) => value,
        None => panic!("2 is nonzero"),
    });

    pub fn new(threads: u16) -> Result<Self, CpuError> {
        let value = NonZeroU16::new(threads).ok_or(CpuError::ZeroThreadsPerCore)?;
        if threads > Self::MAX {
            return Err(CpuError::TooManyThreadsPerCore { max: Self::MAX });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }

    pub const fn smt_enabled(self) -> bool {
        self.0.get() > 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CpuTopology {
    physical_cores: NonZeroU16,
    threads_per_core: ThreadsPerCore,
}

impl CpuTopology {
    pub fn new(physical_cores: u16, threads_per_core: u16) -> Result<Self, CpuError> {
        let cores = NonZeroU16::new(physical_cores).ok_or(CpuError::ZeroPhysicalCores)?;
        let threads = ThreadsPerCore::new(threads_per_core)?;
        Ok(Self::from_parts(cores, threads))
    }

    pub fn with_threads(physical_cores: u16, threads_per_core: ThreadsPerCore) -> Result<Self, CpuError> {
        let cores = NonZeroU16::new(physical_cores).ok_or(CpuError::ZeroPhysicalCores)?;
        Ok(Self::from_parts(cores, threads_per_core))
    }

    fn from_parts(physical_cores: NonZeroU16, threads_per_core: ThreadsPerCore) -> Self {
        Self {
            physical_cores,
            threads_per_core,
        }
    }

    pub const fn physical_cores(&self) -> u16 {
        self.physical_cores.get()
    }

    pub const fn threads_per_core(&self) -> ThreadsPerCore {
        self.threads_per_core
    }

    pub const fn logical_cpus(&self) -> u32 {
        self.physical_cores.get() as u32 * self.threads_per_core.get() as u32
    }

    pub const fn smt_enabled(&self) -> bool {
        self.threads_per_core.smt_enabled()
    }

    pub fn can_host(&self, vcpus: VcpuCount) -> bool {
        u32::from(vcpus.get()) <= self.logical_cpus()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VcpuCount(NonZeroU16);

impl VcpuCount {
    pub const fn new(count: u16) -> Result<Self, CpuError> {
        match NonZeroU16::new(count) {
            Some(value) => Ok(Self(value)),
            None => Err(CpuError::ZeroVcpus),
        }
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CpuAllocation {
    vcpus: VcpuCount,
    topology: CpuTopology,
}

impl CpuAllocation {
    pub fn new(vcpus: VcpuCount, topology: CpuTopology) -> Result<Self, CpuError> {
        let count = u32::from(vcpus.get());
        let logical_cpus = topology.logical_cpus();
        if count > logical_cpus {
            return Err(CpuError::VcpusExceedLogicalCpus {
                vcpus: count,
                logical_cpus,
            });
        }
        Ok(Self { vcpus, topology })
    }

    pub const fn vcpus(&self) -> VcpuCount {
        self.vcpus
    }

    pub const fn topology(&self) -> CpuTopology {
        self.topology
    }

    pub const fn core_equivalent(&self) -> u16 {
        let threads = self.topology.threads_per_core.get() as u32;
        let vcpus = self.vcpus.get() as u32;
        ((vcpus + threads - 1) / threads) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_topology_derives_logical_cpus_from_cores_and_smt_width() {
        let smt = CpuTopology::new(8, 2).expect("valid topology");
        assert_eq!(smt.physical_cores(), 8);
        assert_eq!(smt.threads_per_core(), ThreadsPerCore::TWO);
        assert_eq!(smt.logical_cpus(), 16);
        assert!(smt.smt_enabled());

        let no_smt = CpuTopology::new(8, 1).expect("valid topology");
        assert_eq!(no_smt.logical_cpus(), 8);
        assert!(!no_smt.smt_enabled());
        assert_eq!(no_smt.threads_per_core(), ThreadsPerCore::ONE);
    }

    #[test]
    fn domain_topology_rejects_zero_cores_and_zero_or_excessive_threads() {
        assert_eq!(
            CpuTopology::new(0, 2),
            Err(CpuError::ZeroPhysicalCores)
        );
        assert_eq!(
            CpuTopology::new(4, 0),
            Err(CpuError::ZeroThreadsPerCore)
        );
        assert_eq!(
            CpuTopology::new(4, 17),
            Err(CpuError::TooManyThreadsPerCore { max: 16 })
        );
    }

    #[test]
    fn domain_allocation_bounds_vcpus_by_logical_cpus() {
        let topology = CpuTopology::new(4, 2).expect("valid topology");
        assert_eq!(topology.logical_cpus(), 8);
        assert!(topology.can_host(VcpuCount::new(8).expect("vcpus")));
        assert!(!topology.can_host(VcpuCount::new(9).expect("vcpus")));
        assert_eq!(
            CpuAllocation::new(VcpuCount::new(9).expect("vcpus"), topology),
            Err(CpuError::VcpusExceedLogicalCpus {
                vcpus: 9,
                logical_cpus: 8
            })
        );
    }

    #[test]
    fn domain_allocation_core_equivalent_tracks_smt_width() {
        let vcpus = VcpuCount::new(8).expect("vcpus");
        let smt = CpuAllocation::new(vcpus, CpuTopology::new(8, 2).expect("topology"))
            .expect("allocation");
        let no_smt = CpuAllocation::new(vcpus, CpuTopology::new(16, 1).expect("topology"))
            .expect("allocation");
        assert_eq!(smt.core_equivalent(), 4);
        assert_eq!(no_smt.core_equivalent(), 8);
    }

    #[test]
    fn domain_vcpu_count_cannot_be_zero() {
        assert_eq!(VcpuCount::new(0), Err(CpuError::ZeroVcpus));
        assert_eq!(VcpuCount::new(32).expect("vcpus").get(), 32);
    }
}
