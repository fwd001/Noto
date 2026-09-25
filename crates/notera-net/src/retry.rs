//! 退避与重试预算（docs/PROXY.md §7、SYNC-PROTOCOL §12）。
//!
//! 规范形态：`delay = min(max, base × factor^n) × (1 ± jitter)`，`Retry-After` 优先。
//! jitter 用**可复现**的伪随机（种子上送），这样竞态类测试能钉死时间线
//! （docs/TEST-PLAN.md §"竞态类 flaky 优先用 fake clock / 显式 barrier 消根"）。

use std::time::Duration;

/// 重试策略。
#[derive(Clone, Debug, PartialEq)]
pub struct RetryPolicy {
    pub base: Duration,
    pub factor: f64,
    pub max: Duration,
    /// 抖动比例，0.2 = ±20%。
    pub jitter: f64,
    /// 一轮内重试次数上限（不含首次尝试）。
    pub budget: u32,
}

impl Default for RetryPolicy {
    /// PROXY.md §7：`min(15min, 2s × 1.85^n) × (1±0.2 jitter)`，一轮内最多 3 次。
    fn default() -> Self {
        RetryPolicy {
            base: Duration::from_secs(2),
            factor: 1.85,
            max: Duration::from_secs(15 * 60),
            jitter: 0.2,
            budget: 3,
        }
    }
}

impl RetryPolicy {
    /// 不重试。
    pub fn none() -> RetryPolicy {
        RetryPolicy {
            budget: 0,
            ..Default::default()
        }
    }

    /// 测试用：短基线、无抖动 —— 断言单调递增时不需要容忍随机。
    pub fn deterministic(base_ms: u64, budget: u32) -> RetryPolicy {
        RetryPolicy {
            base: Duration::from_millis(base_ms),
            factor: 1.85,
            max: Duration::from_secs(15 * 60),
            jitter: 0.0,
            budget,
        }
    }

    /// 指数部分（未去抖、已封顶）。
    pub fn capped_delay(&self, attempt: u32) -> Duration {
        let raw = self.base.as_secs_f64() * self.factor.powi(attempt.saturating_sub(1) as i32);
        Duration::from_secs_f64(raw.min(self.max.as_secs_f64()))
    }

    /// 第 `attempt` 次尝试失败后要等多久（`attempt` 从 1 开始）。
    ///
    /// `seed` 决定抖动，因此同一 seed 的两次运行延迟一致。
    pub fn delay_for(&self, attempt: u32, seed: u64) -> Duration {
        let capped = self.capped_delay(attempt);
        if self.jitter <= 0.0 {
            return capped;
        }
        // 确定性抖动：splitmix64(attempt ^ seed) → [-jitter, +jitter]
        let h = splitmix64(attempt as u64 ^ seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let unit = (h >> 11) as f64 / (1u64 << 53) as f64; // [0,1)
        let k = (unit * 2.0 - 1.0) * self.jitter;
        let d = capped.as_secs_f64() * (1.0 + k);
        Duration::from_secs_f64(d.max(0.0).min(self.max.as_secs_f64()))
    }

    /// 解析 `Retry-After`（秒数或 HTTP-日期），失败返回 `None`。
///
/// 秒数形式优先服从（SYNC-PROTOCOL §12："`Retry-After` 存在则优先"）。
pub fn retry_after(value: &str, now_ms: u64) -> Option<Duration> {
    let v = value.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let dt = http_date_secs(v)?;
    let now = (now_ms / 1000) as i64;
    Some(Duration::from_secs((dt - now).max(0) as u64))
    }
}

/// 极简 RFC 1123 解析（`Wed, 21 Oct 2015 07:28:00 GMT`）→ unix 秒。///
/// 不为此拉 chrono 进网络层：只用到一个换算，手写比多一个依赖更可控。
fn http_date_secs(s: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let (_, rest) = s.split_once(',')?;
    let mut it = rest.split_whitespace();
    let day: u64 = it.next()?.parse().ok()?;
    let mon = it.next()?.to_ascii_lowercase();
    let m = MONTHS.iter().position(|x| *x == mon)? as u64 + 1;
    let year: u64 = it.next()?.parse().ok()?;
    let time = it.next()?;
    let mut t = time.split(':');
    let hh: u64 = t.next()?.parse().ok()?;
    let mm: u64 = t.next()?.parse().ok()?;
    let ss: u64 = t.next().unwrap_or("0").parse().ok()?;
    Some(days_from_civil(year as i64, m, day) * 86400 + (hh * 3600 + mm * 60 + ss) as i64)
}

/// Howard Hinnant 的 civil→days 算法（无依赖、无时区歧义）。
fn days_from_civil(y_in: i64, m: u64, d: u64) -> i64 {
    let y = y_in - i64::from(m <= 2);
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m as i64 - 3 } else { m as i64 + 9 };
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_is_monotonic_and_capped() {
        let p = RetryPolicy::deterministic(2000, 8);
        let mut prev = Duration::ZERO;
        for n in 1..=8 {
            let d = p.capped_delay(n);
            assert!(d > prev, "退避必须单调递增: {n}");
            prev = d;
        }
        assert!(p.capped_delay(64) <= p.max, "必须封顶在 max");
        assert_eq!(p.capped_delay(1), Duration::from_secs(2));
    }

    #[test]
    fn jitter_is_reproducible_and_bounded() {
        let p = RetryPolicy {
            jitter: 0.2,
            ..Default::default()
        };
        let a = p.delay_for(3, 42);
        let b = p.delay_for(3, 42);
        assert_eq!(a, b, "同一 seed 的抖动必须可复现");
        let center = p.capped_delay(3);
        let lo = center.mul_f32(0.79);
        let hi = center.mul_f32(1.21);
        assert!(a >= lo && a <= hi, "抖动越界: {a:?} vs {center:?}");
    }

    #[test]
    fn retry_after_accepts_both_forms() {
        assert_eq!(
            RetryPolicy::retry_after("7", 0),
            Some(Duration::from_secs(7))
        );
        let d = RetryPolicy::retry_after("Wed, 21 Oct 2015 07:28:00 GMT", 0);
        assert_eq!(d, Some(Duration::from_secs(1_445_412_480)));
        assert_eq!(RetryPolicy::retry_after("garbage", 0), None);
    }
}
