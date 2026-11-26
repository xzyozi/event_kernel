//! 内部の補助関数。

/// `src` の内容を `dst` へ、短い方の長さだけコピーする。パニックしない。
#[inline]
pub(crate) fn copy_bytes(dst: &mut [u8], src: &[u8]) {
    let len = dst.len().min(src.len());
    if let (Some(dst), Some(src)) = (dst.get_mut(..len), src.get(..len)) {
        dst.copy_from_slice(src);
    }
}