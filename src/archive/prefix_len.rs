/// Trait for prefix length integer types.
pub trait PrefixLen: Copy + From<u8> {
    /// Size in bytes.
    const SIZE: usize;
    /// Alignment in bytes.
    const ALIGN: usize;
    /// Maximum length that can be stored for this prefix length.
    const MAX_LEN: usize;
    /// Convert to `usize`.
    fn into_usize(self) -> usize;
}

impl PrefixLen for u8 {
    const SIZE: usize = 1;
    const ALIGN: usize = 1;
    const MAX_LEN: usize = u8::MAX as usize;

    fn into_usize(self) -> usize { self as usize }
}

impl PrefixLen for u16 {
    const SIZE: usize = 2;
    const ALIGN: usize = 2;
    const MAX_LEN: usize = u16::MAX as usize;

    fn into_usize(self) -> usize { self as usize }
}

impl PrefixLen for u32 {
    const SIZE: usize = 4;
    const ALIGN: usize = 4;
    const MAX_LEN: usize = u32::MAX as usize;

    fn into_usize(self) -> usize { self as usize }
}