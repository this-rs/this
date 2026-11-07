//! The photos bridge, the pure part: the bound on a selection, and the answers the Swift side
//! sends (`ios/Sources/photos.swift`). No Tauri, no native code: it runs on the host.
//!
//! Read-only, and deliberately narrow: the status of the library, and an explicit selection of
//! images the person chose in the system picker. The library itself is never listed here.
//!
//! This is not the device contract (`this_platform::device_bridge`): that one has no photos
//! answer. The error codes and the permission states it shares with the contract are reused,
//! and `limited` (PhotoKit's partial access) is added, because the contract has no such state.

use serde::{Deserialize, Serialize};
use this_platform::device_bridge::DeviceError;

/// The fewest photos a selection may ask for.
pub const PICK_MIN: u32 = 1;
/// The most photos a selection may ask for. The images are held in memory and sent back
/// base64-encoded: this bound keeps one reply within reach of the bridge.
pub const PICK_MAX: u32 = 10;

/// A selection bound outside `PICK_MIN..=PICK_MAX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitError {
    OutOfRange { given: u32 },
}

/// The selection bound, kept in range. Refused rather than clamped: asking for 50 photos and
/// getting 10 without a word would be a silent change of what the person was offered.
pub fn validate_limit(limit: u32) -> Result<u32, LimitError> {
    if (PICK_MIN..=PICK_MAX).contains(&limit) {
        Ok(limit)
    } else {
        Err(LimitError::OutOfRange { given: limit })
    }
}

/// The arguments of the native `photosPick` method.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickRequest {
    pub limit: u32,
}

/// The permission state of the photo library. Same words as the contract's
/// `PermissionState`, plus `limited` (iOS 14 partial access).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryState {
    Granted,
    Limited,
    Denied,
    Prompt,
    Unsupported,
}

/// The only image types the bridge hands over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageMime {
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/png")]
    Png,
}

/// One chosen image: its bytes, base64-encoded. Held in memory for the call only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickedImage {
    pub mime: ImageMime,
    pub data: String,
}

/// What the Swift side answers. `kind` names the variant, as in the device contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PhotosReply {
    /// The images chosen. Empty when the person cancelled the picker.
    Picked {
        items: Vec<PickedImage>,
    },
    Status {
        state: LibraryState,
    },
    Error {
        code: DeviceError,
    },
}

/// The size bound of one image, as the caller receives it (decoded, before base64).
/// Mirrored by `PhotoBytes.maxImage` in `ios/Sources/photos.swift`: the native side shrinks an
/// image below it, and this side refuses one above it. Two checks, one bound.
pub const MAX_IMAGE_BYTES: u64 = 2 * 1024 * 1024;

/// The size bound of a whole answer. Ten images at the per-image bound is 20 MB; above this,
/// the batch is refused rather than sent through the IPC bridge.
pub const MAX_TOTAL_BYTES: u64 = 12 * 1024 * 1024;

/// Checks a picked batch against the transfer bounds.
///
/// `Err` is a [`PhotosReply::Error`] with [`DeviceError::Internal`], the code the bridge already
/// uses for a refused argument: the contract has no code for "too heavy". The caller (the shell)
/// must call this before decoding the images into anything it keeps.
pub fn check_batch(items: &[PickedImage]) -> Result<(), PhotosReply> {
    let mut total: u64 = 0;
    for item in items {
        let Some(bytes) = crate::families::files::decoded_len(&item.data) else {
            return Err(refused());
        };
        if bytes > MAX_IMAGE_BYTES {
            return Err(refused());
        }
        total = total.saturating_add(bytes);
        if total > MAX_TOTAL_BYTES {
            return Err(refused());
        }
    }
    Ok(())
}

fn refused() -> PhotosReply {
    PhotosReply::Error {
        code: DeviceError::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A base64 payload of exactly `bytes` decoded bytes, valid standard base64.
    fn b64(bytes: usize) -> String {
        use std::fmt::Write;
        // The `base64` crate is not a dependency here: three bytes ("abc") encode to "YWJj",
        // so a multiple of three keeps the padding away and the length exact.
        let mut out = String::new();
        for _ in 0..bytes / 3 {
            out.push_str("YWJj");
        }
        let _ = write!(out, "");
        out
    }

    fn image(bytes: usize) -> PickedImage {
        PickedImage {
            mime: ImageMime::Jpeg,
            data: b64(bytes),
        }
    }

    #[test]
    fn a_batch_below_the_bounds_is_accepted() {
        assert_eq!(check_batch(&[]), Ok(()));
        assert_eq!(check_batch(&[image(3 * 1000)]), Ok(()));
        assert_eq!(check_batch(&[image(3 * 1000), image(3 * 1000)]), Ok(()));
    }

    #[test]
    fn an_image_above_the_per_image_bound_is_refused() {
        // One byte over the bound, rounded to a whole base64 group.
        let over = MAX_IMAGE_BYTES as usize + 3;
        assert_eq!(
            check_batch(&[image(over)]),
            Err(PhotosReply::Error {
                code: DeviceError::Internal
            })
        );
    }

    #[test]
    fn a_batch_above_the_total_bound_is_refused_even_when_each_image_fits() {
        // Seven images at the per-image bound: each is fine, the sum is not.
        let at_bound = MAX_IMAGE_BYTES as usize;
        let batch: Vec<PickedImage> = (0..7).map(|_| image(at_bound)).collect();
        assert_eq!(
            check_batch(&batch),
            Err(PhotosReply::Error {
                code: DeviceError::Internal
            })
        );
    }

    #[test]
    fn a_payload_that_is_not_base64_is_refused_not_measured_as_zero() {
        let bad = PickedImage {
            mime: ImageMime::Png,
            data: "not base64 !!".to_string(),
        };
        assert_eq!(
            check_batch(&[bad]),
            Err(PhotosReply::Error {
                code: DeviceError::Internal
            })
        );
    }

    #[test]
    fn the_bounds_are_one_to_ten() {
        assert_eq!(validate_limit(PICK_MIN), Ok(1));
        assert_eq!(validate_limit(5), Ok(5));
        assert_eq!(validate_limit(PICK_MAX), Ok(10));
    }

    #[test]
    fn a_bound_outside_the_range_is_refused_not_clamped() {
        for given in [0, 11, 50, u32::MAX] {
            assert_eq!(
                validate_limit(given),
                Err(LimitError::OutOfRange { given }),
                "{given}"
            );
        }
    }

    #[test]
    fn the_pick_request_is_the_native_payload() {
        let value = serde_json::to_value(PickRequest { limit: 3 }).unwrap();
        assert_eq!(value, json!({ "limit": 3 }));
    }

    #[test]
    fn a_picked_reply_decodes_from_the_swift_shape() {
        let reply: PhotosReply = serde_json::from_str(
            r#"{"kind":"picked","items":[{"mime":"image/jpeg","data":"/9j/4A=="},{"mime":"image/png","data":"iVBORw0K"}]}"#,
        )
        .unwrap();
        assert_eq!(
            reply,
            PhotosReply::Picked {
                items: vec![
                    PickedImage {
                        mime: ImageMime::Jpeg,
                        data: "/9j/4A==".into()
                    },
                    PickedImage {
                        mime: ImageMime::Png,
                        data: "iVBORw0K".into()
                    },
                ]
            }
        );
    }

    #[test]
    fn a_cancelled_pick_is_an_empty_list_not_an_error() {
        let reply: PhotosReply = serde_json::from_str(r#"{"kind":"picked","items":[]}"#).unwrap();
        assert_eq!(reply, PhotosReply::Picked { items: vec![] });
    }

    #[test]
    fn every_library_state_the_native_side_sends_is_known() {
        for (wire, state) in [
            ("granted", LibraryState::Granted),
            ("limited", LibraryState::Limited),
            ("denied", LibraryState::Denied),
            ("prompt", LibraryState::Prompt),
            ("unsupported", LibraryState::Unsupported),
        ] {
            let json = format!(r#"{{"kind":"status","state":"{wire}"}}"#);
            let reply: PhotosReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, PhotosReply::Status { state }, "{wire}");
        }
    }

    #[test]
    fn every_error_code_the_photos_side_sends_is_a_contract_code() {
        for (wire, code) in [
            ("not_supported", DeviceError::NotSupported),
            ("unavailable", DeviceError::Unavailable),
            ("internal", DeviceError::Internal),
        ] {
            let json = format!(r#"{{"kind":"error","code":"{wire}"}}"#);
            let reply: PhotosReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, PhotosReply::Error { code }, "{wire}");
        }
    }

    #[test]
    fn an_image_type_outside_jpeg_and_png_is_refused() {
        let json = r#"{"kind":"picked","items":[{"mime":"image/heic","data":"AAAA"}]}"#;
        assert!(serde_json::from_str::<PhotosReply>(json).is_err());
    }

    #[test]
    fn a_picked_reply_round_trips() {
        let reply = PhotosReply::Picked {
            items: vec![PickedImage {
                mime: ImageMime::Jpeg,
                data: "/9j/".into(),
            }],
        };
        let value = serde_json::to_value(&reply).unwrap();
        assert_eq!(
            value,
            json!({"kind": "picked", "items": [{"mime": "image/jpeg", "data": "/9j/"}]})
        );
        let back: PhotosReply = serde_json::from_value(value).unwrap();
        assert_eq!(back, reply);
    }
}
