//! Query length metrics shared by typed regex predicates and result scoring.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TextMetrics {
    pub(super) utf16_len: usize,
    pub(super) scalar_len: usize,
}

impl TextMetrics {
    pub(super) fn from_str(text: &str) -> Self {
        if text.is_ascii() {
            let len = text.len();
            return Self {
                utf16_len: len,
                scalar_len: len,
            };
        }

        text.chars().fold(
            Self {
                utf16_len: 0,
                scalar_len: 0,
            },
            |mut metrics, scalar| {
                metrics.utf16_len += scalar.len_utf16();
                metrics.scalar_len += 1;
                metrics
            },
        )
    }
}
