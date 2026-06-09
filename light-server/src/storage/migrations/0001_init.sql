PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value BLOB NOT NULL
);

INSERT OR IGNORE INTO meta(key, value) VALUES
  ('schema_version', '1'),
  ('scope', 'p2tr-sp'),
  ('network', 'mainnet'),
  ('finality_depth', '6'),
  ('suggested_reorg_cache_depth', '24'),
  ('checkpoint_interval', '10000');

CREATE TABLE IF NOT EXISTS blocks (
  height INTEGER PRIMARY KEY,
  block_hash BLOB NOT NULL UNIQUE,
  previous_block_hash BLOB NOT NULL,
  block_time INTEGER,
  p2tr_created_count INTEGER NOT NULL DEFAULT 0,
  p2tr_spent_count INTEGER NOT NULL DEFAULT 0,
  anchor_last_uid INTEGER NOT NULL CHECK(anchor_last_uid >= 0)
);

CREATE INDEX IF NOT EXISTS blocks_hash_idx
ON blocks(block_hash);

CREATE TABLE IF NOT EXISTS block_stats (
  height INTEGER PRIMARY KEY,
  tx_count INTEGER NOT NULL,
  output_count_total INTEGER NOT NULL,
  p2tr_output_count INTEGER NOT NULL,
  p2tr_sp_candidate_count INTEGER NOT NULL,
  p2tr_nums_count INTEGER NOT NULL,
  p2tr_reused_count INTEGER NOT NULL,
  p2tr_excluded_by_scope_count INTEGER NOT NULL,
  indexed_output_count INTEGER NOT NULL,
  indexed_spent_count INTEGER NOT NULL,
  tx_with_p2tr_output_count INTEGER NOT NULL,
  tx_with_indexed_output_count INTEGER NOT NULL,
  tweak_count INTEGER NOT NULL,
  FOREIGN KEY(height) REFERENCES blocks(height)
);

CREATE TABLE IF NOT EXISTS block_exclusion_stats (
  height INTEGER NOT NULL,
  reason TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY(height, reason),
  FOREIGN KEY(height) REFERENCES blocks(height)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS profiles (
  profile_id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  scope TEXT NOT NULL,
  cutthrough_blocks INTEGER NOT NULL,
  materialization_interval_blocks INTEGER NOT NULL,
  served_tip_height INTEGER NOT NULL DEFAULT 0,
  served_tip_hash BLOB,
  enabled INTEGER NOT NULL DEFAULT 1,
  UNIQUE(scope, cutthrough_blocks)
);

INSERT OR IGNORE INTO profiles
  (profile_id, name, scope, cutthrough_blocks, materialization_interval_blocks)
VALUES
  (1, 'raw-sp',      'p2tr-sp', 0,      1),
  (2, 'ct12-sp',     'p2tr-sp', 12,     144),
  (3, 'ct144-sp',    'p2tr-sp', 144,    144),
  (4, 'ct1008-sp',   'p2tr-sp', 1008,   144),
  (5, 'ct4320-sp',   'p2tr-sp', 4320,   144),
  (6, 'ct12960-sp',  'p2tr-sp', 12960,  144),
  (7, 'ct52560-sp',  'p2tr-sp', 52560,  144),
  (8, 'ct105120-sp', 'p2tr-sp', 105120, 144);


-- Canonical-chain UTXO lookup used only by the raw indexer to derive BIP352
CREATE TABLE IF NOT EXISTS p2tr_outputs (
  uid INTEGER PRIMARY KEY CHECK(uid > 0),
  txid BLOB NOT NULL,
  created_height INTEGER NOT NULL,
  created_block_hash BLOB NOT NULL,
  tx_index INTEGER NOT NULL,
  vout INTEGER NOT NULL,
  value_sat INTEGER NOT NULL,
  script_pubkey BLOB NOT NULL,
  p2tr_xonly_key BLOB NOT NULL,
  is_nums INTEGER NOT NULL DEFAULT 0,
  is_reused INTEGER NOT NULL DEFAULT 0,
  reuse_count_at_creation INTEGER NOT NULL DEFAULT 1,
  FOREIGN KEY(created_height) REFERENCES blocks(height)
);

CREATE INDEX IF NOT EXISTS p2tr_outputs_created_height_idx
ON p2tr_outputs(created_height);

CREATE INDEX IF NOT EXISTS p2tr_outputs_created_order_idx
ON p2tr_outputs(created_height, tx_index, vout);

CREATE UNIQUE INDEX IF NOT EXISTS p2tr_outputs_location_idx
ON p2tr_outputs(created_height, created_block_hash, tx_index, vout);

CREATE TABLE IF NOT EXISTS p2tr_utxo_lookup (
  txid BLOB NOT NULL,
  vout INTEGER NOT NULL,
  uid INTEGER NOT NULL CHECK(uid > 0),
  value_sat INTEGER NOT NULL,
  script_pubkey BLOB NOT NULL,
  p2tr_xonly_key BLOB NOT NULL,
  created_height INTEGER NOT NULL,
  created_block_hash BLOB NOT NULL,
  created_tx_index INTEGER NOT NULL,
  PRIMARY KEY(txid, vout),
  FOREIGN KEY(uid) REFERENCES p2tr_outputs(uid)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS p2tr_utxo_lookup_uid_idx
ON p2tr_utxo_lookup(uid);

CREATE TABLE IF NOT EXISTS p2tr_spends (
  uid INTEGER PRIMARY KEY CHECK(uid > 0),
  spent_height INTEGER NOT NULL,
  spent_block_hash BLOB NOT NULL,
  spend_tx_index INTEGER NOT NULL,
  FOREIGN KEY(uid) REFERENCES p2tr_outputs(uid),
  FOREIGN KEY(spent_height) REFERENCES blocks(height)
);

CREATE INDEX IF NOT EXISTS p2tr_spends_spent_height_idx
ON p2tr_spends(spent_height);

CREATE INDEX IF NOT EXISTS p2tr_spends_spent_order_idx
ON p2tr_spends(spent_height, uid);

CREATE INDEX IF NOT EXISTS p2tr_spends_uid_spent_idx
ON p2tr_spends(uid, spent_height);

CREATE TABLE IF NOT EXISTS p2tr_key_stats (
  output_key BLOB PRIMARY KEY,
  first_height INTEGER NOT NULL,
  last_height INTEGER NOT NULL,
  seen_count INTEGER NOT NULL,
  first_uid INTEGER,
  last_uid INTEGER,
  is_nums INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS tx_tweaks (
  height INTEGER NOT NULL,
  tx_index INTEGER NOT NULL,
  tweak BLOB NOT NULL,
  PRIMARY KEY(height, tx_index),
  FOREIGN KEY(height) REFERENCES blocks(height)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS payload_cache (
  profile_id INTEGER NOT NULL,
  height INTEGER NOT NULL,
  block_hash BLOB NOT NULL,
  payload BLOB NOT NULL,
  payload_len INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY(profile_id, height),
  FOREIGN KEY(profile_id) REFERENCES profiles(profile_id),
  FOREIGN KEY(height) REFERENCES blocks(height)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS checkpoint_cache (
  profile_id INTEGER NOT NULL,
  height INTEGER NOT NULL,
  block_hash BLOB NOT NULL,
  checkpoint BLOB NOT NULL,
  checkpoint_len INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY(profile_id, height),
  FOREIGN KEY(profile_id) REFERENCES profiles(profile_id),
  FOREIGN KEY(height) REFERENCES blocks(height)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS cutthrough_snapshot_cache (
  profile_id INTEGER NOT NULL,
  height INTEGER NOT NULL,
  block_hash BLOB NOT NULL,
  payload BLOB NOT NULL,
  payload_len INTEGER NOT NULL,
  block_count INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY(profile_id, height),
  FOREIGN KEY(profile_id) REFERENCES profiles(profile_id),
  FOREIGN KEY(height) REFERENCES blocks(height)
) WITHOUT ROWID;
