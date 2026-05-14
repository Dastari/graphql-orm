# Digitise Extraction Notes

Digitise currently has storage code in:

- `/home/toby/digitse/src/storage/mod.rs`
- `/home/toby/digitse/src/storage/local.rs`
- `/home/toby/digitse/src/media/mod.rs`
- `/home/toby/digitse/src/domain/entities/media.rs`

The generic pieces extracted into this crate are:

- storage backend enum
- namespace enum
- stored object metadata
- object storage trait
- storage service wrapper
- key generation
- local filesystem backend
- checksum generation

The Digitise-specific pieces intentionally left out are:

- `Storage` and `Media` entity definitions
- collection ownership
- upload authorization
- download authorization
- content classification
- MIME sniffing
- thumbnail generation
- document preview generation
- audit events
- AI analysis hooks

Digitise adoption should replace its local storage module with this crate, then keep `MediaService` as the application service that classifies content, writes `Storage` rows, creates optional `Media` rows, and queues derivative work.
