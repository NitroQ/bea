-- Bea v1 local-first schema. Runtime initialization is idempotent and mirrors this migration.
PRAGMA foreign_keys=ON;
PRAGMA journal_mode=WAL;

CREATE TABLE IF NOT EXISTS meetings (id TEXT PRIMARY KEY, title TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, duration_seconds INTEGER NOT NULL DEFAULT 0, language TEXT NOT NULL DEFAULT 'auto', asr_engine_id TEXT NOT NULL DEFAULT 'qwen-standard');
CREATE TABLE IF NOT EXISTS transcript_segments (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, start_seconds INTEGER NOT NULL, end_seconds INTEGER NOT NULL, text TEXT NOT NULL, language_detected TEXT, language_confidence REAL);
CREATE VIRTUAL TABLE IF NOT EXISTS transcript_fts USING fts5(meeting_id UNINDEXED, segment_id UNINDEXED, text);
CREATE TABLE IF NOT EXISTS recording_chunks (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, ordinal INTEGER NOT NULL, start_seconds INTEGER NOT NULL, end_seconds INTEGER NOT NULL, path TEXT NOT NULL, state TEXT NOT NULL, UNIQUE(meeting_id, ordinal));
CREATE TABLE IF NOT EXISTS jobs (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, kind TEXT NOT NULL, state TEXT NOT NULL, progress REAL NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0, error TEXT);
CREATE TABLE IF NOT EXISTS media_sources (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, path TEXT NOT NULL, kind TEXT NOT NULL, duration_seconds INTEGER, copied INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS model_manifests (id TEXT PRIMARY KEY, name TEXT NOT NULL, version TEXT NOT NULL, size_bytes INTEGER NOT NULL, sha256 TEXT NOT NULL, runtime TEXT NOT NULL, languages TEXT NOT NULL, installed INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS provider_configs (id TEXT PRIMARY KEY, kind TEXT NOT NULL, base_url TEXT NOT NULL, model TEXT NOT NULL, credential_ref TEXT, enabled INTEGER NOT NULL DEFAULT 1);
CREATE TABLE IF NOT EXISTS usage_records (id TEXT PRIMARY KEY, provider_id TEXT NOT NULL, model TEXT NOT NULL, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL, estimated_cost REAL, operation TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS ledger_events (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, payload TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS minutes (meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE, payload TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS participants (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, name TEXT NOT NULL, role TEXT, created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS context_events (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, kind TEXT NOT NULL, payload TEXT NOT NULL, confidence REAL NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS topics (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, label TEXT NOT NULL, summary TEXT, start_seconds INTEGER, end_seconds INTEGER);
CREATE TABLE IF NOT EXISTS visual_evidence (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, timestamp_seconds INTEGER NOT NULL, path TEXT NOT NULL, thumbnail_path TEXT, ocr_text TEXT, perceptual_hash TEXT NOT NULL, description TEXT);
CREATE TABLE IF NOT EXISTS action_items (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, event_id TEXT, owner TEXT, summary TEXT NOT NULL, due TEXT, status TEXT NOT NULL DEFAULT 'open', FOREIGN KEY(event_id) REFERENCES context_events(id) ON DELETE SET NULL);
CREATE TABLE IF NOT EXISTS llm_runs (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, provider_id TEXT, model TEXT NOT NULL, operation TEXT NOT NULL, prompt_version TEXT NOT NULL, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL, state TEXT NOT NULL, error TEXT, created_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_context_events_meeting ON context_events(meeting_id);
CREATE INDEX IF NOT EXISTS idx_visual_evidence_meeting_time ON visual_evidence(meeting_id, timestamp_seconds);
CREATE INDEX IF NOT EXISTS idx_action_items_meeting_status ON action_items(meeting_id, status);

CREATE TABLE IF NOT EXISTS speaker_names (meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, speaker_index INTEGER NOT NULL, name TEXT NOT NULL, UNIQUE(meeting_id, speaker_index));
