-- Extensions required by the schema.
--   vector   : pgvector, for chunk embeddings and HNSW search
--   pgcrypto : gen_random_uuid(), used as the fallback id default (clients
--              normally supply UUIDv7 ids)
create extension if not exists vector;
create extension if not exists pgcrypto;
