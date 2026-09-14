//! Resource-neutral composition of CPU and memory pressure folds.

use crate::{
    CpuPressure, CpuPressurePolicy, CpuPressureSample, CpuPressureTransition, MemoryPressure,
    MemoryPressurePolicy, PressureSample, PressureTransition,
};

/// Live pressure folds for one caller-owned resource entry.
#[derive(Debug, Default)]
pub struct PressureMonitor {
    cpu: CpuPressure,
    memory: MemoryPressure,
}

/// Durable edges produced by one pair of readings.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PressureEdges {
    pub cpu: Option<CpuPressureTransition>,
    pub memory: Option<PressureTransition>,
}

impl PressureMonitor {
    pub fn evaluate(
        &mut self,
        cpu: &CpuPressureSample,
        memory: &PressureSample,
        cpu_policy: &CpuPressurePolicy,
        memory_policy: &MemoryPressurePolicy,
    ) -> PressureEdges {
        PressureEdges {
            cpu: self.cpu.evaluate(cpu, cpu_policy),
            memory: self.memory.evaluate(memory, memory_policy),
        }
    }

    pub fn abandon(&mut self, at: std::time::Instant) -> PressureEdges {
        PressureEdges {
            cpu: self.cpu.abandon(at),
            memory: self.memory.abandon(at),
        }
    }
}
