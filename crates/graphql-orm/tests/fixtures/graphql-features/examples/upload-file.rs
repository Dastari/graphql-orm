fn main() {
    let upload = graphql_orm::async_graphql::UploadValue {
        filename: "file.bin".into(),
        content_type: None,
        content: tempfile::tempfile().unwrap(),
    };
    let _: std::fs::File = upload.content;
}
