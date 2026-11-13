//! 连接延迟的平滑显示。
//!
//! 单次往返时间抖动很大，直接显示会让数字跳个不停。这里用指数移动平均：
//! 新样本占 30%，既能在几次采样内跟上真实变化，又不会被一次偶然的卡顿带偏。

use std::time::Duration;

const SMOOTHING: f64 = 0.3;

/// 延迟所处的档位，界面按它着色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatencyLevel {
    Good,
    Fair,
    Poor,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LatencyMeter {
    smoothed_ms: Option<f64>,
    /// 最近一次探测没有在时限内得到回应。
    timed_out: bool,
}

impl LatencyMeter {
    /// 记录一次探测结果；None 表示超时。
    pub fn record(&mut self, sample: Option<Duration>) {
        match sample {
            Some(sample) => {
                let sample_ms = sample.as_secs_f64() * 1000.0;
                // 超时之后的第一个样本直接采用：之前的平均值已经过时。
                self.smoothed_ms = Some(match self.smoothed_ms {
                    Some(previous) if !self.timed_out => {
                        previous + SMOOTHING * (sample_ms - previous)
                    }
                    _ => sample_ms,
                });
                self.timed_out = false;
            }
            None => self.timed_out = true,
        }
    }

    pub fn timed_out(&self) -> bool {
        self.timed_out
    }

    /// 平滑后的延迟，取整到毫秒；还没有样本时为 None。
    pub fn millis(&self) -> Option<u64> {
        self.smoothed_ms.map(|ms| ms.round() as u64)
    }

    pub fn level(&self) -> Option<LatencyLevel> {
        if self.timed_out {
            return Some(LatencyLevel::Poor);
        }
        self.millis().map(|ms| match ms {
            0..100 => LatencyLevel::Good,
            100..250 => LatencyLevel::Fair,
            _ => LatencyLevel::Poor,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Option<Duration> {
        Some(Duration::from_millis(value))
    }

    #[test]
    fn starts_empty_and_takes_first_sample_as_is() {
        let mut meter = LatencyMeter::default();
        assert_eq!(meter.millis(), None);
        assert_eq!(meter.level(), None);
        meter.record(ms(40));
        assert_eq!(meter.millis(), Some(40));
        assert_eq!(meter.level(), Some(LatencyLevel::Good));
    }

    #[test]
    fn one_spike_does_not_dominate() {
        let mut meter = LatencyMeter::default();
        meter.record(ms(20));
        meter.record(ms(520));
        assert_eq!(meter.millis(), Some(170));
        meter.record(ms(20));
        assert_eq!(meter.millis(), Some(125));
    }

    #[test]
    fn follows_a_real_change_within_a_few_samples() {
        let mut meter = LatencyMeter::default();
        meter.record(ms(20));
        for _ in 0..8 {
            meter.record(ms(300));
        }
        assert!(meter.millis().unwrap() > 280);
        assert_eq!(meter.level(), Some(LatencyLevel::Poor));
    }

    #[test]
    fn timeout_marks_poor_and_resets_the_average() {
        let mut meter = LatencyMeter::default();
        meter.record(ms(20));
        meter.record(None);
        assert!(meter.timed_out());
        assert_eq!(meter.level(), Some(LatencyLevel::Poor));
        meter.record(ms(200));
        assert!(!meter.timed_out());
        assert_eq!(meter.millis(), Some(200));
    }
}
