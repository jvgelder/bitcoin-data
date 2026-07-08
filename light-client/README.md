# Silent Payments light client UI

React + Tailwind browser wallet UI for the `light-server` HTTP API.

The UI is split into layers:

```text
src/api/        HTTP client and binary range-frame parsing
src/crypto/     bitcoinjs-lib ECC initialization with the official tiny-secp256k1 WASM browser package
src/keys/       Silent Payment key import/generation/descriptor/backup helpers
src/state/      hook-based global state provider and state update helpers
src/components  Tailwind UI components only
```

## Wallet flow

The client now opens with a wallet-oriented flow instead of a technical UID dashboard:

1. Generate a new Silent Payment wallet: private scan key + private spend key, then download a backup.
2. Import watch-only key material: private scan key + public spend key.
3. Import a generated backup, encrypted backup, Silent Payment descriptor, or full private key material. BIP392-style `sp(spscan...)`, `sp(spspend...)`, and simple `sp(private_scan, spend)` forms are accepted by the prototype parser.
4. Import an existing wallet backup/history JSON. The import accepts labels, transactions, optional key material, and optional last online height.

After watch-only or descriptor/private-key import, the user is asked whether to apply cut-through. The toggle includes a tooltip explaining that cut-through can ignore outputs created and spent inside the selected sync window.

After history import, if no last online height is present, the user is asked for the last date/time the wallet was online. The UI estimates a scan start height around that time and automatically enables cut-through.

## Wallet screen

After setup, the wallet screen shows:

- balance at the top,
- Send and Receive buttons,
- recent transactions,
- clickable transaction rows with amount, fee, fiat estimate, date, txid, and label name.

The top-right menu contains:

```text
Backup
Labels
Settings
Import
```

## Labels

The label editor uses folder-like paths rooted at `/`, such as:

```text
/client/invoice
/change
```

Each label keeps a stable numeric id from 1 to 100. Editing or moving the label changes the path only; the id remains the same. Label id `1` is displayed as `change` in transaction views.

## Backup and import

Backup exports key material, labels, sync metadata, and imported transactions/history. Exports can be encrypted with AES-256-GCM using PBKDF2-HMAC-SHA256.

The import page accepts:

```text
sp(spscan1q...)
sp(spspend1q...)
generated key=value backups
encrypted generated backups with passphrase
wallet backup JSON
```

The wallet-history JSON format is intentionally small and inspired by the wallet metadata backup draft linked in the issue, but it is not claiming final BIP compatibility while that draft is still evolving.

Keys are kept in browser memory only. The app does not write private key material to local storage.


## Provider settings

Settings now has separate provider lists for:

- light servers, used for `/health`, `/manifest`, `/tip`, and `/blocks/light`;
- Esplora-compatible full-block providers, used later when a light-payload hit must be confirmed against the raw Bitcoin block.

Defaults are:

```text
Light server: http://127.0.0.1:3000
Blockstream Esplora: https://blockstream.info/api
mempool.space: https://mempool.space/api
```

Each provider can be enabled/disabled, selected as primary, assigned an API key, and configured to send that key as a header, bearer token, or query parameter. Enabled providers are tried in primary-first fallback order. Each configured provider can also be tested separately: light servers can test health/tip and light-block download, while Esplora-compatible providers can test height lookup and raw-block download.

## Light sync implementation status

Implemented:

- Connects to `GET /health`, `GET /manifest`, and `GET /tip`.
- Syncs bounded ranges with `GET /blocks/light?start={height}&count={n}`.
- Supports `labels`, `filter_reuse`, `cutthrough`, `cutthrough_start`, `cutthrough_tip`, and `max_bytes` query parameters.
- Requests JSON (`Accept: application/json`) so the browser can inspect decoded light blocks without a Cap'n Proto JavaScript schema.
- Includes a binary `BDSR` range-frame parser in `src/api/rangeFrame.ts` for later Cap'n Proto decoding.
- Verifies range ordering and payload chain continuity using `height`, `block_hash`, and `previous_block_hash`.

Not implemented yet:

- Full BIP352 scanner matching.
- Wiring full-block confirmation into wallet-state updates. The API/provider layer for Esplora-compatible raw-block fallback is present, but the scanner still needs to call it on matches.
- Automatic local signing for the direct Send button. It still needs confirmed UTXO signing metadata from the scanner/import layer.
- Live fiat price lookup.

## Run

```bash
cd light-client
corepack enable
pnpm install
pnpm dev
```

By default, the light-server provider is `http://127.0.0.1:3000`. For a remote server, open Settings and set the direct light-server URL. The remote server must allow browser requests from the client origin.

## Run with Podman

```bash
cd light-client
podman compose up --build dev
```

Open:

```text
http://127.0.0.1:5173
```

If host port `5173` is already in use:

```bash
LIGHT_CLIENT_HOST_PORT=5174 podman compose up --build dev
```

Open:

```text
http://127.0.0.1:5174
```

Usually leave `LIGHT_CLIENT_CONTAINER_PORT` at its default. Configure the light server and block providers from the in-app Settings page.

## Build

```bash
pnpm build
```

## Send and receive flow

The wallet dashboard now opens dedicated `Send` and `Receive` pages.

Send flow:

- scan or paste a `bitcoin:` URI,
- scan or paste a BIP73 payment request URL,
- type a p2tr, p2wpkh, p2wsh, p2sh, or p2pkh address plus an amount in sats or BTC with up to 6 decimals,
- BIP73 URLs are fetched with `Accept: text/uri-list`, and the first returned Bitcoin URI is used,
- when pasting a `bitcoin:` URI, automatically fill the amount and note/message from the request,
- review destination address, amount, note, selected label, and fee rate,
- create an unsigned PSBT,
- parse signed PSBTs from base64 or hex using PSBT magic-byte detection,
- show the unsigned PSBT as a QR and as base64 text,
- scan or paste a signed PSBT only,
- verify the signed PSBT outputs match the original PSBT output plan before broadcast,
- inspect PSBTv2/BIP375 Silent Payment ECDH-share and DLEQ fields when a signed PSBT is scanned,
- broadcast through the configured Esplora provider fallback list using `POST /tx`.

BIP370/BIP375 support is currently parser/inspector-level: the app recognizes PSBTv2 and the BIP375 global, input, and output fields, but it does not yet construct a BIP375 Silent Payment send PSBT. Sending to `sp1...`/`tsp1...` recipients therefore fails safely until the BIP375 constructor/signer path is complete.

The direct `Send` button is wired to the same transaction plan path but intentionally fails safely until the scanner/import layer supplies confirmed spendable UTXOs and the derived Silent Payment spend keys needed for local signing. The PSBT path is the intended hardware/offline signer flow first.

Receive flow:

- select a wallet label,
- generate the BIP352 Silent Payment address from the wallet public scan key and labeled public spend key,
- show the address as a QR and as text.

The `/change` label is not offered as a receive label. It is reserved for wallet change handling.

## Demo mode

Demo mode can be enabled from the start screen or from Settings. It loads a deterministic demo Silent Payment wallet, example transactions, and fake spendable UTXOs so Send, Receive, PSBT display, label-based coin selection, and transaction detail screens can be exercised before the real scanner/import path has produced wallet-owned UTXOs.

In demo mode:

- the transaction list contains clickable sample transactions;
- Settings can try to fetch a recent Esplora block sample and turn a few public block outputs into fake wallet UTXOs, falling back to local demo examples;
- Receive shows a valid demo Silent Payment address for the selected label;
- Send can create unsigned PSBTs from fake UTXOs grouped by wallet label, including UTXOs derived from the downloaded block sample when available;
- a synthetic unconfirmed RBF transaction spends one of those fake demo UTXOs;
- the RBF panel can build a replacement by subtracting extra fee from the change output. Demo RBF replacements are recorded locally and are not broadcast to the network.

Demo UTXOs and demo RBF transactions are not wallet-owned coins and should not be treated as spendable mainnet funds.

## Send flow

The Send page is split into three modes: Scan, Enter, and Paste.

- Scan detects bitcoin URIs, BIP73 URLs, Silent Payment addresses, and PSBT v1/v2 content.
- Enter accepts a standard Bitcoin address or Silent Payment address plus an amount.
- Paste accepts a raw address, bitcoin URI, BIP73/payment URL, Silent Payment address, or PSBT text.

The composer shows slow, fast, and custom fee-rate choices, plus spendable outputs grouped by stable label id. Label 1 is displayed as `/change`.

The PSBT flow creates an unsigned PSBT, displays it as QR/base64, then accepts a signed PSBT or raw transaction. Before broadcast, the client verifies that the final transaction outputs match the original payment plan.


Labels menu:

- uses `/` paths, for example `/change` or `/client/invoice`;
- each label row shows current spendable balance, total transaction volume, net delta, and current confirmed UTXO count;
- label ids remain stable when paths are edited or moved.
