//! 标识符。
//!
//! 一个 128 位、按时间可排序、文本形态定长 26 字符的 ID：**48 位毫秒 + 80 位熵**，
//! Crockford base32 编码（不含 I/L/O/U，抄错的概率低一档）。
//!
//! 为什么要可排序：事件在存储里按键排序扫描，ID 可排序意味着"按写入顺序读回来"
//! 不需要第二个索引。为什么不引第三方库：这点东西的形状不值得再拉一条依赖进来，
//! 而它的正确性是可以逐条测出来的。

use std::fmt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const ENCODED_LEN: usize = 26;

/// 128 位标识符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id([u8; 16]);

impl Id {
    /// 新建一个：当前毫秒 + 80 位熵。
    ///
    /// 同一毫秒内多次调用**严格递增**——熵的低位挂了一个进程内计数器，
    /// 所以"同一毫秒里写了两行"不会退化成两个无序的 ID。
    #[must_use]
    pub fn generate() -> Self {
        let millis = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis()),
        )
        .unwrap_or(u64::MAX)
            & 0x0000_FFFF_FFFF_FFFF;
        Self::from_parts(millis, entropy())
    }

    /// 由毫秒与 80 位熵拼出来。超出各自位宽的部分被丢掉。测试用它造确定的 ID。
    #[must_use]
    pub fn from_parts(millis: u64, entropy: u128) -> Self {
        let value = (u128::from(millis & 0x0000_FFFF_FFFF_FFFF) << 80)
            | (entropy & 0x0000_FFFF_FFFF_FFFF_FFFF_FFFF);
        Self(value.to_be_bytes())
    }

    /// 生成它的那一毫秒。
    #[must_use]
    pub fn millis(self) -> u64 {
        u64::try_from(u128::from_be_bytes(self.0) >> 80).unwrap_or(u64::MAX)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// 从 26 个字符解析回来。
    ///
    /// # Errors
    /// 长度不对、出现字母表以外的字符、或者高位溢出了 128 位。
    pub fn parse(text: &str) -> Result<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != ENCODED_LEN {
            return Err(Error::invalid(format!(
                "ID 应当是 {ENCODED_LEN} 个字符，收到 {}",
                bytes.len()
            )));
        }
        let mut value: u128 = 0;
        for (index, byte) in bytes.iter().enumerate() {
            let digit = decode_digit(*byte).ok_or_else(|| {
                Error::invalid(format!(
                    "ID 第 {} 位不是合法字符：{}",
                    index + 1,
                    *byte as char
                ))
            })?;
            // 26 * 5 = 130 位，头一个字符只允许占 3 位，否则就溢出了。
            if index == 0 && digit > 7 {
                return Err(Error::invalid("ID 溢出 128 位"));
            }
            value = (value << 5) | u128::from(digit);
        }
        Ok(Self(value.to_be_bytes()))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = u128::from_be_bytes(self.0);
        let mut out = [0u8; ENCODED_LEN];
        for (index, slot) in out.iter_mut().enumerate() {
            let shift = 5 * (ENCODED_LEN - 1 - index);
            let digit = if shift >= 128 {
                usize::try_from(value >> 125).unwrap_or(0)
            } else {
                usize::try_from((value >> shift) & 0x1F).unwrap_or(0)
            };
            *slot = ALPHABET[digit];
        }
        // out 里每一个字节都来自 ALPHABET，因而必然是 ASCII。
        f.write_str(std::str::from_utf8(&out).unwrap_or("<非法 ID>"))
    }
}

impl Serialize for Id {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

fn decode_digit(byte: u8) -> Option<u8> {
    let upper = byte.to_ascii_uppercase();
    ALPHABET
        .iter()
        .position(|candidate| *candidate == upper)
        .and_then(|index| u8::try_from(index).ok())
}

/// 80 位熵：进程内一次性种子 + 单调计数器，过一遍 splitmix64。
///
/// 这不是密码学随机数，也不需要是：它防的是同一毫秒内的碰撞，不是猜测。
fn entropy() -> u128 {
    static SEED: OnceLock<u64> = OnceLock::new();
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let seed = *SEED.get_or_init(|| {
        let nanos = u64::from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos()),
        );
        let stack = std::ptr::from_ref(&nanos) as u64;
        splitmix64(nanos ^ stack.rotate_left(17) ^ 0x9E37_79B9_7F4A_7C15)
    });
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    // ⚠️ 顺序不能反：**计数器占高 48 位，散列占低 32 位**。
    // 反过来写，同一毫秒内的序就由散列决定，也就是随机 —— "同毫秒严格递增"当场作废。
    (u128::from(counter & 0x0000_FFFF_FFFF_FFFF) << 32)
        | u128::from(splitmix64(seed ^ counter) & 0xFFFF_FFFF)
}

fn splitmix64(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 文本形态定长且可往返() {
        let id = Id::from_parts(1_700_000_000_000, 0x0123_4567_89AB_CDEF_0123);
        let text = id.to_string();
        assert_eq!(text.len(), ENCODED_LEN);
        assert_eq!(Id::parse(&text).unwrap(), id);
    }

    #[test]
    fn 按时间可排序() {
        let early = Id::from_parts(1_000, u128::MAX);
        let late = Id::from_parts(1_001, 0);
        assert!(early < late, "毫秒更小的 ID 必须排在前面，哪怕它的熵更大");
        assert!(
            early.to_string() < late.to_string(),
            "文本形态的序要与二进制形态一致"
        );
    }

    #[test]
    fn 同一毫秒内严格递增() {
        let ids: Vec<String> = (0..1_000).map(|_| Id::generate().to_string()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "同一毫秒内生成的 ID 必须已经是有序的");
        sorted.dedup();
        assert_eq!(sorted.len(), 1_000, "不允许重复");
    }

    #[test]
    fn 毫秒取得回来() {
        assert_eq!(
            Id::from_parts(1_700_000_000_000, 7).millis(),
            1_700_000_000_000
        );
    }

    #[test]
    fn 坏输入被拒() {
        assert!(Id::parse("").is_err());
        assert!(Id::parse("0123456789ABCDEFGHJKMNPQR").is_err(), "少一位");
        assert!(Id::parse("0123456789ABCDEFGHJKMNPQRST").is_err(), "多一位");
        assert!(
            Id::parse("U123456789ABCDEFGHJKMNPQRS").is_err(),
            "U 不在字母表里"
        );
        assert!(
            Id::parse("ZZZZZZZZZZZZZZZZZZZZZZZZZZ").is_err(),
            "溢出 128 位"
        );
    }

    #[test]
    fn 小写也认() {
        let id = Id::generate();
        assert_eq!(Id::parse(&id.to_string().to_lowercase()).unwrap(), id);
    }

    #[test]
    fn 序列化成字符串而不是字节数组() {
        let id = Id::from_parts(1_000, 2);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<Id>(&json).unwrap(), id);
    }
}
