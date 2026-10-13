use super::*;
use serde_json::json;

fn png() -> Vec<u8> {
    let image = image::DynamicImage::new_rgba8(2, 3);
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).unwrap();
    bytes.into_inner()
}
fn image_reference(bytes: &[u8]) -> AiToolImageReference {
    serde_json::from_value(json!({
        "reference":Uuid::new_v4(),"sha256":format!("{:x}",Sha256::digest(bytes)),
        "mime":"image/png","width":2,"height":3,
        "captured_at_ms":1000,"action_expires_at_ms":1010,"retained_until_ms":2000
    }))
    .unwrap()
}
fn resource(reference: &AiToolImageReference) -> AiToolMediaResourceId {
    AiToolMediaResourceId::new("preview-v1", reference.reference()).unwrap()
}
#[test]
fn genuine_pixels_are_decoded_and_bytes_are_redacted() {
    let bytes = png();
    let reference = image_reference(&bytes);
    let image =
        AiResolvedToolImage::new(reference.clone(), resource(&reference), bytes.clone()).unwrap();
    assert_eq!(image.byte_count(), bytes.len() as u64);
    assert_eq!(image.bytes(), bytes);
    let debug = format!("{image:?}");
    assert!(!debug.contains(&format!("{bytes:?}")));
    assert!(!reference.action_label_is_current(1500));
    image
        .validate_policy(
            "preview-v1",
            AiToolResultMediaLimits::new(1, 4096, 10, 100, 1000, 100).unwrap(),
            1500,
        )
        .unwrap();
    assert!(image
        .validate_policy(
            "preview-v1",
            AiToolResultMediaLimits::new(1, 4096, 10, 100, 1000, 100).unwrap(),
            2000
        )
        .is_err());
}
#[test]
fn wrong_digest_dimensions_mime_and_resource_are_rejected() {
    let bytes = png();
    let reference = image_reference(&bytes);
    for (field, value) in [
        ("sha256", json!("0".repeat(64))),
        ("width", json!(3)),
        ("mime", json!("image/jpeg")),
    ] {
        let mut metadata = serde_json::to_value(&reference).unwrap();
        metadata[field] = value;
        let wrong: AiToolImageReference = serde_json::from_value(metadata).unwrap();
        assert!(
            AiResolvedToolImage::new(wrong.clone(), resource(&wrong), bytes.clone()).is_err(),
            "{field}"
        );
    }
    assert!(AiResolvedToolImage::new(
        reference.clone(),
        AiToolMediaResourceId::new("preview-v1", Uuid::new_v4()).unwrap(),
        bytes
    )
    .is_err());
}
#[test]
fn actual_decode_and_narrow_policy_limits_are_required() {
    let bytes = b"not an image";
    let reference = image_reference(bytes);
    assert!(AiResolvedToolImage::new(reference.clone(), resource(&reference), &bytes[..]).is_err());
    let bytes = png();
    let reference = image_reference(&bytes);
    let image = AiResolvedToolImage::new(reference.clone(), resource(&reference), bytes).unwrap();
    assert!(image
        .validate_policy(
            "other-v1",
            AiToolResultMediaLimits::new(1, 4096, 10, 100, 1000, 100).unwrap(),
            1500
        )
        .is_err());
    assert!(image
        .validate_policy(
            "preview-v1",
            AiToolResultMediaLimits::new(1, image.byte_count() - 1, 10, 100, 1000, 100).unwrap(),
            1500
        )
        .is_err());
    assert!(image
        .validate_policy(
            "preview-v1",
            AiToolResultMediaLimits::new(1, 4096, 2, 5, 1000, 100).unwrap(),
            1500
        )
        .is_err());
}
#[tokio::test]
async fn bounded_decode_preserves_exact_immutable_payload() {
    let bytes = png();
    let reference = image_reference(&bytes);
    let decoded = AiResolvedToolImage::decode_bounded(
        reference.clone(),
        resource(&reference),
        bytes.clone(),
        AiToolResultMediaLimits::new(1, 4096, 10, 100, 1000, 5000).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(decoded.bytes(), bytes);
    assert!(AiResolvedToolImage::decode_bounded(
        reference.clone(),
        resource(&reference),
        bytes,
        AiToolResultMediaLimits::new(1, 4096, 1, 100, 1000, 5000).unwrap()
    )
    .await
    .is_err());
}

#[test]
fn two_genuine_pngs_must_fit_the_complete_result_pixel_budget() {
    let bytes = png();
    let first = image_reference(&bytes);
    let second = image_reference(&bytes);
    let one = AiResolvedToolImage::new(first.clone(), resource(&first), bytes.clone()).unwrap();
    let two = AiResolvedToolImage::new(second.clone(), resource(&second), bytes).unwrap();
    let limits = AiToolResultMediaLimits::new(2, 8192, 10, 6, 1000, 5000).unwrap();
    one.validate_policy("preview-v1", limits, 1500).unwrap();
    two.validate_policy("preview-v1", limits, 1500).unwrap();
    assert!(crate::tool_media::validate_image_reference_batch(
        &[first.clone(), second.clone()],
        limits
    )
    .is_err());
    crate::tool_media::validate_image_reference_batch(std::slice::from_ref(&first), limits)
        .unwrap();
    crate::tool_media::validate_image_reference_batch(
        &[first, second],
        AiToolResultMediaLimits::new(2, 8192, 10, 12, 1000, 5000).unwrap(),
    )
    .unwrap();
}
