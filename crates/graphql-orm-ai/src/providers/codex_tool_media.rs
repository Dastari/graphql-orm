//! Codec/protocol tests. Owning source and budget admission is tested separately in SQLite.
use super::tests::{model_request, turn};
use super::*;
use sha2::{Digest, Sha256};
fn genuine_image() -> crate::AiResolvedToolImage {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 3)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    let id = uuid::Uuid::new_v4();
    let reference:crate::AiToolImageReference=serde_json::from_value(json!({"reference":id,"sha256":hex::encode(Sha256::digest(&bytes)),"mime":"image/png","width":2,"height":3,"captured_at_ms":1000,"action_expires_at_ms":1010,"retained_until_ms":2000})).unwrap();
    crate::AiResolvedToolImage::new(
        reference,
        crate::AiToolMediaResourceId::new("preview-v1", id).unwrap(),
        bytes,
    )
    .unwrap()
}
#[test]
fn reopened_original_pixels_are_real_image_input_and_keep_original_action_label() {
    let image = genuine_image();
    let mut input = turn();
    input.tool_images = vec![image.clone()];
    let items = input.protocol_input().unwrap();
    assert_eq!(items[0], json!({"type":"text","text":"hello"}));
    let label: Value = serde_json::from_str(items[1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(label["capturedAtMs"], 1000);
    assert_eq!(label["actionExpiresAtMs"], 1010);
    assert_eq!(label["actionEvidenceRenewed"], false);
    assert_eq!(items[2]["type"], "image");
    assert_eq!(
        items[2]["url"],
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(image.bytes())
        )
    );
    assert!(!items[1]["text"].as_str().unwrap().contains("base64"));
    let debug = format!("{input:?}");
    assert!(!debug.contains("data:image"));
}
#[test]
fn json_reference_is_text_only_and_unadmitted_image_batches_are_refused() {
    let mut request = model_request();
    request.input = vec![ModelInputBlock::Text {
        text: json!({"ImageReference":uuid::Uuid::new_v4(),"ImageMimeType":"image/png"})
            .to_string(),
    }];
    let input = AiCodexAppServerTurnInput::try_from_model_request(request).unwrap();
    assert!(
        input
            .protocol_input()
            .unwrap()
            .iter()
            .all(|item| item["type"] == "text")
    );
    let mut input = turn();
    input.tool_images = vec![genuine_image(); 5];
    assert!(input.protocol_input().is_err());
}
