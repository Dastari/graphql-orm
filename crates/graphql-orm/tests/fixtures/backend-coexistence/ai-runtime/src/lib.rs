#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use graphql_orm_ai::{AiGraphqlToolManifest, AiGraphqlToolManifestSet, AiToolCatalog};

    #[test]
    fn mssql_producer_and_sqlite_runtime_share_the_canonical_manifest() {
        let (sdl, produced) = legacy_service::ai_tool_manifest().expect("compile Legacy manifest");
        let payload = produced.extension_payload().expect("encode extension");
        let decoded = AiGraphqlToolManifest::from_extension_payload(payload)
            .expect("SQLite runtime decodes canonical producer payload");

        assert_eq!(decoded.fingerprint, produced.fingerprint);
        assert_eq!(
            serde_json::to_vec(&decoded).expect("encode decoded manifest"),
            serde_json::to_vec(&produced).expect("encode produced manifest")
        );

        let set = AiGraphqlToolManifestSet::aggregate(
            [decoded],
            &BTreeMap::from([("legacy-service".to_owned(), sdl)]),
        )
        .expect("aggregate exact active schema");
        let mut catalog = AiToolCatalog::new();
        set.register_into(&mut catalog)
            .expect("register canonical manifest");
        assert_eq!(catalog.descriptors().count(), 2);
    }
}
