//! Checked wire-offset helpers used by generated codecs.
//!
//! Group counts, entry block lengths and var data lengths come from the
//! wire and are untrusted. Generated decoders and encoders route every
//! offset derived from them through these helpers, so a truncated or
//! corrupted frame surfaces as an `Err` instead of an out-of-bounds read
//! or write.

use crate::buffer::ReadBuffer;
use crate::decoder::DecodeError;
use crate::error::EncodeError;
use crate::header::GroupHeader;

/// Returns `start + len` if those bytes lie within a buffer of
/// `available` bytes.
///
/// # Errors
/// Returns [`DecodeError::VarPartOutOfBounds`] for `part` when the span
/// extends past the buffer or `start + len` overflows.
#[inline]
pub fn span_end(
    available: usize,
    start: usize,
    len: usize,
    part: &'static str,
) -> Result<usize, DecodeError> {
    match start.checked_add(len) {
        Some(end) if end <= available => Ok(end),
        end => Err(out_of_bounds(part, end, available)),
    }
}

/// Reads the group header at `pos` and checks it lies within `buffer`.
///
/// # Errors
/// Returns [`DecodeError::VarPartOutOfBounds`] for `part` when the header
/// extends past the buffer.
#[inline]
pub fn group_header_at(
    buffer: &[u8],
    pos: usize,
    part: &'static str,
) -> Result<GroupHeader, DecodeError> {
    span_end(buffer.len(), pos, GroupHeader::ENCODED_LENGTH, part)?;
    Ok(GroupHeader::wrap(buffer, pos))
}

/// Reads the var data field whose length header (1, 2 or 4 bytes wide)
/// starts at `pos`, and returns its payload plus the offset just past it.
///
/// # Errors
/// Returns [`DecodeError::VarPartOutOfBounds`] for `part` when the length
/// header or the payload it announces extends past the buffer.
#[inline]
pub fn var_data_at<'a>(
    buffer: &'a [u8],
    pos: usize,
    header_length: usize,
    part: &'static str,
) -> Result<(&'a [u8], usize), DecodeError> {
    let len = match header_length {
        1 => buffer.try_get_u8(pos).map(usize::from),
        2 => buffer.try_get_u16_le(pos).map(usize::from),
        _ => buffer
            .try_get_u32_le(pos)
            .and_then(|len| usize::try_from(len).ok()),
    };
    let header_end = span_end(buffer.len(), pos, header_length, part)?;
    let Some(len) = len else {
        return Err(out_of_bounds(part, Some(header_end), buffer.len()));
    };
    let end = span_end(buffer.len(), header_end, len, part)?;
    let payload = buffer
        .get(header_end..end)
        .ok_or_else(|| out_of_bounds(part, Some(end), buffer.len()))?;
    Ok((payload, end))
}

/// Checks that a write needing `required` bytes fits a buffer of
/// `available` bytes.
///
/// `required` is `None` when computing it overflowed.
///
/// # Errors
/// Returns [`EncodeError::BufferTooShort`] for `field` when it does not fit.
#[inline]
pub fn ensure_capacity(
    available: usize,
    required: Option<usize>,
    field: &'static str,
) -> Result<(), EncodeError> {
    match required {
        Some(required) if required <= available => Ok(()),
        _ => Err(buffer_too_short(field, required, available)),
    }
}

#[cold]
#[inline(never)]
fn out_of_bounds(part: &'static str, required: Option<usize>, available: usize) -> DecodeError {
    DecodeError::VarPartOutOfBounds {
        part,
        required: required.unwrap_or(usize::MAX),
        available,
    }
}

#[cold]
#[inline(never)]
fn buffer_too_short(field: &'static str, required: Option<usize>, available: usize) -> EncodeError {
    EncodeError::BufferTooShort {
        field,
        required: required.unwrap_or(usize::MAX),
        available,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PART: &str = "var data field 'note'";

    #[test]
    fn test_span_end_in_bounds_returns_end() {
        assert_eq!(span_end(10, 4, 6, PART), Ok(10));
        assert_eq!(span_end(10, 10, 0, PART), Ok(10));
    }

    #[test]
    fn test_span_end_past_buffer_returns_err() {
        assert_eq!(
            span_end(10, 4, 7, PART),
            Err(DecodeError::VarPartOutOfBounds {
                part: PART,
                required: 11,
                available: 10,
            })
        );
    }

    #[test]
    fn test_span_end_overflow_returns_err_with_max() {
        assert_eq!(
            span_end(10, usize::MAX, 1, PART),
            Err(DecodeError::VarPartOutOfBounds {
                part: PART,
                required: usize::MAX,
                available: 10,
            })
        );
    }

    #[test]
    fn test_group_header_at_reads_header() {
        let buf = [8u8, 0, 3, 0];
        let header = group_header_at(&buf, 0, "repeating group 'legs'");
        assert!(matches!(
            header,
            Ok(GroupHeader {
                block_length: 8,
                num_in_group: 3,
            })
        ));
    }

    #[test]
    fn test_group_header_at_truncated_returns_err() {
        let buf = [8u8, 0, 3];
        assert_eq!(
            group_header_at(&buf, 0, "repeating group 'legs'").err(),
            Some(DecodeError::VarPartOutOfBounds {
                part: "repeating group 'legs'",
                required: 4,
                available: 3,
            })
        );
    }

    #[test]
    fn test_var_data_at_each_header_width_returns_payload() {
        let buf = [2u8, b'a', b'b', 9];
        assert_eq!(var_data_at(&buf, 0, 1, PART), Ok((&b"ab"[..], 3)));
        let buf = [2u8, 0, b'a', b'b'];
        assert_eq!(var_data_at(&buf, 0, 2, PART), Ok((&b"ab"[..], 4)));
        let buf = [1u8, 0, 0, 0, b'z'];
        assert_eq!(var_data_at(&buf, 0, 4, PART), Ok((&b"z"[..], 5)));
    }

    #[test]
    fn test_var_data_at_truncated_header_returns_err() {
        let buf = [2u8];
        assert_eq!(
            var_data_at(&buf, 0, 2, PART),
            Err(DecodeError::VarPartOutOfBounds {
                part: PART,
                required: 2,
                available: 1,
            })
        );
    }

    #[test]
    fn test_var_data_at_truncated_payload_returns_err() {
        let buf = [5u8, 0, b'a'];
        assert_eq!(
            var_data_at(&buf, 0, 2, PART),
            Err(DecodeError::VarPartOutOfBounds {
                part: PART,
                required: 7,
                available: 3,
            })
        );
    }

    #[test]
    fn test_ensure_capacity_fits_returns_ok() {
        assert_eq!(ensure_capacity(10, Some(10), PART), Ok(()));
    }

    #[test]
    fn test_ensure_capacity_too_short_or_overflow_returns_err() {
        assert_eq!(
            ensure_capacity(10, Some(11), PART),
            Err(EncodeError::BufferTooShort {
                field: PART,
                required: 11,
                available: 10,
            })
        );
        assert_eq!(
            ensure_capacity(10, None, PART),
            Err(EncodeError::BufferTooShort {
                field: PART,
                required: usize::MAX,
                available: 10,
            })
        );
    }
}
