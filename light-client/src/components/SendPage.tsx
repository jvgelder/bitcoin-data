import { useEffect, useMemo, useState } from 'react';
import {
  broadcastTransactionWithFallback,
  fetchFeeEstimatesWithFallback,
  type BlockProviderConfig,
} from '../api/blockProviderApi';
import { makeDemoChangeAddress } from '../demo/demoWallet';
import {
  detectPaymentInput,
  parseBitcoinUri,
  parseManualAddressPayment,
  paymentRequestDisplaySource,
  resolveScannedPaymentRequest,
  scriptTypeLabel,
} from '../payments/paymentRequest';
import {
  createUnsignedDemoRawTransaction,
  createUnsignedPsbtPlan,
  describePsbtInput,
  signPsbtLocallyOrThrow,
  verifySignedPsbtOutputs,
  type UnsignedPsbtPlan,
} from '../payments/psbt';
import { QrCode } from '../qr/QrCode';
import { QrScanner } from '../qr/QrScanner';
import { useLightClientActions, useLightClientState } from '../state/LightClientProvider';
import { enabledProviders, isOpenSpendableUtxo } from '../state/lightClientState';
import type { PendingPaymentRequest, WalletSpendableUtxo, WalletTransaction } from '../state/types';
import { formatBtcFromSats, formatSats } from '../utils/format';
import { Button, Card, Field, Input, PrimaryButton, Select } from './ui';

type SendStep = 'compose' | 'review' | 'psbt' | 'scan-signed' | 'ready-broadcast';
type ComposeMode = 'scan' | 'enter' | 'paste';
type FeeMode = 'slow' | 'fast' | 'custom';
type AmountUnit = 'btc' | 'sats';

const DEFAULT_FEES = { slow: 3, fast: 15 };

export function SendPage({ onDone }: { onDone(): void }) {
  const state = useLightClientState();
  const actions = useLightClientActions();
  const [step, setStep] = useState<SendStep>('compose');
  const [composeMode, setComposeMode] = useState<ComposeMode>('scan');
  const [payment, setPayment] = useState<PendingPaymentRequest>();
  const [pasteText, setPasteText] = useState('');
  const [enterAddress, setEnterAddress] = useState('');
  const [amountInput, setAmountInput] = useState('');
  const [amountUnit, setAmountUnit] = useState<AmountUnit>('sats');
  const [note, setNote] = useState('');
  const [feeMode, setFeeMode] = useState<FeeMode>('slow');
  const [customFeeRate, setCustomFeeRate] = useState(5);
  const [feeRates, setFeeRates] = useState(DEFAULT_FEES);
  const [spendFromLabelId, setSpendFromLabelId] = useState<number | 'all'>('all');
  const [changeAddress, setChangeAddress] = useState(state.demo.enabled ? makeDemoChangeAddress() : '');
  const [psbtPlan, setPsbtPlan] = useState<UnsignedPsbtPlan>();
  const [signedTx, setSignedTx] = useState<{ rawTxHex: string; txid: string }>();
  const [signedPsbtText, setSignedPsbtText] = useState('');
  const [message, setMessage] = useState<{ tone: 'success' | 'error' | 'info'; text: string }>();

  const blockProviders = useMemo<BlockProviderConfig[]>(() => {
    return enabledProviders(state.providers.blockProviders, state.providers.activeBlockProviderId).map((provider) => ({
      name: provider.name,
      url: provider.url,
      apiKey: provider.apiKey,
      apiKeyPlacement: provider.apiKeyPlacement,
      apiKeyName: provider.apiKeyName,
    }));
  }, [state.providers]);

  useEffect(() => {
    const controller = new AbortController();
    fetchFeeEstimatesWithFallback(blockProviders, controller.signal)
      .then(({ fees }) => {
        const slow = Math.max(1, Math.ceil(fees['6'] ?? fees['12'] ?? DEFAULT_FEES.slow));
        const fast = Math.max(slow, Math.ceil(fees['1'] ?? fees['2'] ?? DEFAULT_FEES.fast));
        setFeeRates({ slow, fast });
      })
      .catch(() => setFeeRates(DEFAULT_FEES));
    return () => controller.abort();
  }, [blockProviders]);

  const spendableGroups = useMemo(() => groupUtxosByLabel(state.spendableUtxos, state.labels), [state.spendableUtxos, state.labels]);
  const selectedUtxos = useMemo(() => {
    if (spendFromLabelId === 'all') return state.spendableUtxos.filter(isOpenSpendableUtxo);
    return state.spendableUtxos.filter((utxo) => isOpenSpendableUtxo(utxo) && utxo.labelId === spendFromLabelId);
  }, [spendFromLabelId, state.spendableUtxos]);
  const feeRate = feeMode === 'custom' ? customFeeRate : feeRates[feeMode];
  const pasteDetection = useMemo(() => detectPaymentInput(pasteText), [pasteText]);
  const enterDetection = useMemo(() => detectPaymentInput(enterAddress), [enterAddress]);

  useEffect(() => {
    const text = pasteText.trim();
    if (!/^bitcoin:/i.test(text)) return;
    try {
      hydratePaymentFields(parseBitcoinUri(text));
    } catch {
      // Ignore partial URI edits while the user is still typing/pasting.
    }
  }, [pasteText]);

  function hydratePaymentFields(request: PendingPaymentRequest): void {
    if (request.amountSat != null) {
      setAmountUnit('sats');
      setAmountInput(String(request.amountSat));
    }
    const requestNote = request.message ?? request.label;
    if (requestNote) setNote(requestNote);
  }

  function updateAmountUnit(nextUnit: AmountUnit): void {
    if (nextUnit === amountUnit) return;
    const clean = amountInput.trim();
    if (clean) {
      try {
        if (amountUnit === 'sats' && nextUnit === 'btc') {
          if (!/^\d+$/.test(clean)) throw new Error('Invalid sats amount.');
          setAmountInput(formatBtcSixFromSats(Number(clean)));
        } else if (amountUnit === 'btc' && nextUnit === 'sats') {
          setAmountInput(String(btcSixToSats(clean)));
        }
      } catch {
        // Keep the original input while the user fixes it.
      }
    }
    setAmountUnit(nextUnit);
  }

  async function handleScan(value: string) {
    try {
      const detection = detectPaymentInput(value);
      if (detection.kind === 'psbt') {
        setMessage({ tone: 'info', text: `Scanned ${describePsbtInput(value)}. Use this after creating an unsigned PSBT.` });
        return;
      }
      const resolved = await resolveScannedPaymentRequest(value);
      hydratePaymentFields(resolved);
      setPayment(withOptionalAmount(resolved));
      setStep('review');
      setMessage(undefined);
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  async function handlePasteReview() {
    try {
      const resolved = await resolveScannedPaymentRequest(pasteText);
      hydratePaymentFields(resolved);
      setPayment(withOptionalAmount(resolved));
      setStep('review');
      setMessage(undefined);
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  function handleEnterReview() {
    try {
      const parsed = parseManualAddressPayment(enterAddress, amountInputToBtcString(amountInput, amountUnit), note);
      setPayment(parsed);
      setStep('review');
      setMessage(undefined);
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  function createPlan(): UnsignedPsbtPlan | undefined {
    if (!payment) return undefined;
    const plan = createUnsignedPsbtPlan({
      payment,
      utxos: selectedUtxos,
      changeAddress: changeAddress.trim() || undefined,
      feeRateSatVb: feeRate,
      networkName: state.manifest?.network,
    });
    setPsbtPlan(plan);
    return plan;
  }

  async function sendDirect() {
    try {
      const plan = createPlan();
      if (!plan) return;
      const rawTxHex = signPsbtLocallyOrThrow(plan);
      const result = await broadcastTransactionWithFallback(blockProviders, rawTxHex);
      recordSentTransaction(result.txid, plan.feeSat, rawTxHex);
      setMessage({ tone: 'success', text: `Broadcast through ${result.providerName}: ${result.txid}` });
      onDone();
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  function showPsbt() {
    try {
      createPlan();
      setStep('psbt');
      setMessage(undefined);
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  function handleSignedScan(value: string) {
    try {
      if (!psbtPlan) throw new Error('Create the unsigned PSBT first.');
      const verified = verifySignedPsbtOutputs(value, psbtPlan.expectedOutputs, state.manifest?.network);
      setSignedTx({ rawTxHex: verified.rawTxHex, txid: verified.txid });
      setStep('ready-broadcast');
      setMessage({ tone: 'success', text: `Signed transaction verified. Outputs match. Txid: ${verified.txid}${verified.psbtInspection ? ` (${verified.psbtInspection})` : ''}` });
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  async function broadcastSigned() {
    try {
      if (!signedTx || !psbtPlan) throw new Error('Scan and verify the signed PSBT first.');
      const result = await broadcastTransactionWithFallback(blockProviders, signedTx.rawTxHex);
      recordSentTransaction(result.txid, psbtPlan.feeSat, signedTx.rawTxHex);
      setMessage({ tone: 'success', text: `Broadcast through ${result.providerName}: ${result.txid}` });
      onDone();
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  function recordSentTransaction(txid: string, feeSat: number, rawTxHex?: string, noteOverride?: string) {
    if (!payment?.amountSat) return;
    const tx: WalletTransaction = {
      id: txid,
      txid,
      direction: 'sent',
      amountSat: payment.amountSat,
      feeSat,
      dateTime: new Date().toISOString(),
      labelId: spendFromLabelId === 'all' ? selectedUtxos[0]?.labelId ?? 1 : spendFromLabelId,
      confirmations: 0,
      rawTxHex,
      rbfChangeOutputIndex: rawTxHex ? 1 : undefined,
      note: noteOverride ?? payment.message,
    };
    actions.addWalletTransaction(tx);
    const spentIds = new Set(selectedUtxos.map((utxo) => utxo.id));
    actions.setSpendableUtxos(state.spendableUtxos.map((utxo) => spentIds.has(utxo.id) ? { ...utxo, reservedByTxid: txid } : utxo));
  }

  function withOptionalAmount(request: PendingPaymentRequest): PendingPaymentRequest {
    if (request.amountSat != null || !amountInput.trim()) return request;
    return {
      ...request,
      amountSat: parseManualAddressPayment(
        request.address,
        amountInputToBtcString(amountInput, amountUnit),
        request.message ?? note,
        false,
      ).amountSat,
      message: request.message ?? (note.trim() || undefined),
    };
  }

  return (
    <div className="space-y-5">
      <Card title="Send" subtitle="Choose scan, enter, or paste. The same detector handles standard Bitcoin addresses, bitcoin URIs, BIP73 URLs, Silent Payment addresses, and PSBT scans.">
        {step === 'compose' ? (
          <div className="space-y-5">
            <div className="grid gap-2 sm:grid-cols-3">
              {(['scan', 'enter', 'paste'] as ComposeMode[]).map((mode) => (
                <Button key={mode} className={composeMode === mode ? 'border-indigo-500 bg-indigo-950 text-indigo-100' : ''} onClick={() => setComposeMode(mode)}>
                  {mode === 'scan' ? 'Scan' : mode === 'enter' ? 'Enter' : 'Paste'}
                </Button>
              ))}
            </div>

            {composeMode === 'scan' ? (
              <section className="rounded-2xl border border-slate-800 bg-slate-950/40 p-4">
                <h3 className="font-semibold text-slate-100">Scan QR</h3>
                <p className="mt-1 text-sm text-slate-400">Detects bitcoin URIs, BIP73 payment URLs, standard addresses, Silent Payment addresses, and PSBT v1/v2 data.</p>
                <div className="mt-4"><QrScanner onScan={(value) => void handleScan(value)} /></div>
              </section>
            ) : null}

            {composeMode === 'enter' ? (
              <section className="rounded-2xl border border-slate-800 bg-slate-950/40 p-4">
                <h3 className="font-semibold text-slate-100">Enter payment</h3>
                <div className="mt-4 grid gap-3">
                  <Field label="Recipient address">
                    <Input value={enterAddress} onChange={(event) => setEnterAddress(event.target.value)} placeholder="bc1p..., bc1q..., 3..., 1..., sp1..." />
                  </Field>
                  <p className="text-xs text-slate-500">Detected: {enterDetection.summary}</p>
                  <PaymentOptions
                    amountInput={amountInput}
                    setAmountInput={setAmountInput}
                    amountUnit={amountUnit}
                    setAmountUnit={updateAmountUnit}
                    note={note}
                    setNote={setNote}
                    feeMode={feeMode}
                    setFeeMode={setFeeMode}
                    feeRates={feeRates}
                    customFeeRate={customFeeRate}
                    setCustomFeeRate={setCustomFeeRate}
                    spendFromLabelId={spendFromLabelId}
                    setSpendFromLabelId={setSpendFromLabelId}
                    spendableGroups={spendableGroups}
                    selectedUtxos={selectedUtxos}
                  />
                  <div className="flex justify-end"><PrimaryButton onClick={handleEnterReview}>Review</PrimaryButton></div>
                </div>
              </section>
            ) : null}

            {composeMode === 'paste' ? (
              <section className="rounded-2xl border border-slate-800 bg-slate-950/40 p-4">
                <h3 className="font-semibold text-slate-100">Paste request</h3>
                <Field label="Address, bitcoin URI, BIP73 URL, Silent Payment address, or PSBT">
                  <textarea value={pasteText} onChange={(event) => setPasteText(event.target.value)} rows={5} className="w-full rounded-xl border border-slate-700 bg-slate-950/70 px-3 py-2 font-mono text-xs text-slate-100" />
                </Field>
                <p className="mt-2 text-xs text-slate-500">Detected: {pasteDetection.summary}</p>
                <div className="mt-4">
                  <PaymentOptions
                    amountInput={amountInput}
                    setAmountInput={setAmountInput}
                    amountUnit={amountUnit}
                    setAmountUnit={updateAmountUnit}
                    note={note}
                    setNote={setNote}
                    feeMode={feeMode}
                    setFeeMode={setFeeMode}
                    feeRates={feeRates}
                    customFeeRate={customFeeRate}
                    setCustomFeeRate={setCustomFeeRate}
                    spendFromLabelId={spendFromLabelId}
                    setSpendFromLabelId={setSpendFromLabelId}
                    spendableGroups={spendableGroups}
                    selectedUtxos={selectedUtxos}
                  />
                </div>
                <div className="mt-4 flex justify-end"><PrimaryButton onClick={() => void handlePasteReview()}>Review</PrimaryButton></div>
              </section>
            ) : null}
          </div>
        ) : null}

        {payment && step !== 'compose' ? (
          <div className="space-y-4">
            <div className="rounded-2xl border border-slate-800 bg-slate-950/50 p-4">
              <div className="text-xs uppercase tracking-wide text-slate-500">Payment request</div>
              <div className="mt-2 break-all font-mono text-sm text-slate-100">{payment.address}</div>
              <div className="mt-3 grid gap-3 sm:grid-cols-4">
                <Info label="Amount" value={payment.amountSat == null ? 'No amount' : formatSats(payment.amountSat)} />
                <Info label="Type" value={scriptTypeLabel(payment.detectedType)} />
                <Info label="Source" value={paymentRequestDisplaySource(payment.source)} />
                <Info label="Fee" value={`${feeRate} sat/vB (${feeMode})`} />
              </div>
            </div>
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="Spend from label">
                <Select value={String(spendFromLabelId)} onChange={(event) => setSpendFromLabelId(event.target.value === 'all' ? 'all' : Number(event.target.value))}>
                  <option value="all">All labels · {formatSats(totalUtxoValue(state.spendableUtxos))}</option>
                  {spendableGroups.map((group) => <option key={group.labelId} value={group.labelId}>{group.name} · {formatSats(group.totalSat)}</option>)}
                </Select>
              </Field>
              <Field label="Change address" hint={state.demo.enabled ? 'Demo mode pre-fills a valid fake change address.' : 'Required until scanner-derived change is wired.'}>
                <Input value={changeAddress} onChange={(event) => setChangeAddress(event.target.value)} placeholder="bc1p... or bc1q..." />
              </Field>
            </div>
            {step === 'review' ? (
              <div className="flex flex-wrap justify-end gap-2">
                <Button onClick={() => setStep('compose')}>Start over</Button>
                <Button onClick={showPsbt}>PSBT</Button>
                <PrimaryButton onClick={() => void sendDirect()}>Send</PrimaryButton>
              </div>
            ) : null}
          </div>
        ) : null}

        {step === 'psbt' && psbtPlan ? (
          <div className="mt-5 space-y-4">
            <div className="grid gap-5 lg:grid-cols-[auto_1fr]">
              <QrCode value={psbtPlan.psbtBase64} title="Unsigned PSBT" />
              <div className="space-y-3">
                <Info label="Selected inputs" value={String(psbtPlan.selectedUtxos.length)} />
                <Info label="Estimated fee" value={formatSats(psbtPlan.feeSat)} />
                <Info label="Change" value={formatSats(psbtPlan.changeSat)} />
                <Field label="Unsigned PSBT base64">
                  <textarea readOnly rows={7} value={psbtPlan.psbtBase64} className="w-full rounded-xl border border-slate-700 bg-slate-950/70 px-3 py-2 font-mono text-xs text-slate-100" />
                </Field>
                <div className="flex justify-end"><PrimaryButton onClick={() => setStep('scan-signed')}>Next: scan signed PSBT</PrimaryButton></div>
              </div>
            </div>
          </div>
        ) : null}

        {step === 'scan-signed' ? (
          <div className="mt-5 space-y-4">
            <h3 className="font-semibold text-slate-100">Scan signed PSBT</h3>
            <p className="text-sm text-slate-400">Scan or paste the signed PSBT only. The client accepts PSBT base64 or PSBT hex, verifies that outputs match the original unsigned PSBT, then enables broadcast.</p>
            <QrScanner onScan={handleSignedScan} />
            <Field label="Paste signed PSBT">
              <textarea rows={5} value={signedPsbtText} onChange={(event) => setSignedPsbtText(event.target.value)} className="w-full rounded-xl border border-slate-700 bg-slate-950/70 px-3 py-2 font-mono text-xs text-slate-100" />
            </Field>
            <div className="flex justify-end"><PrimaryButton onClick={() => handleSignedScan(signedPsbtText)}>Verify signed PSBT</PrimaryButton></div>
          </div>
        ) : null}

        {step === 'ready-broadcast' ? (
          <div className="mt-5 rounded-2xl border border-emerald-800 bg-emerald-950/30 p-4">
            <p className="text-sm text-emerald-100">Signed PSBT verified. Outputs match the original payment plan.</p>
            <div className="mt-4 flex justify-end"><PrimaryButton className="border-emerald-500 bg-emerald-500 hover:bg-emerald-400" onClick={() => void broadcastSigned()}>Send transaction</PrimaryButton></div>
          </div>
        ) : null}

        {message ? <p className={messageClass(message.tone)}>{message.text}</p> : null}
      </Card>
    </div>
  );
}

function PaymentOptions(props: {
  amountInput: string;
  setAmountInput(value: string): void;
  amountUnit: AmountUnit;
  setAmountUnit(value: AmountUnit): void;
  note: string;
  setNote(value: string): void;
  feeMode: FeeMode;
  setFeeMode(value: FeeMode): void;
  feeRates: { slow: number; fast: number };
  customFeeRate: number;
  setCustomFeeRate(value: number): void;
  spendFromLabelId: number | 'all';
  setSpendFromLabelId(value: number | 'all'): void;
  spendableGroups: SpendableGroup[];
  selectedUtxos: WalletSpendableUtxo[];
}) {
  return (
    <div className="grid gap-3">
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={props.amountUnit === 'sats' ? 'Amount sats' : 'Amount BTC'} hint="Use sats for exact values, or BTC in millionth-BTC increments. Pasted URI amounts are filled automatically.">
          <div className="grid gap-2 sm:grid-cols-[1fr_auto]">
            <Input
              value={props.amountInput}
              inputMode={props.amountUnit === 'sats' ? 'numeric' : 'decimal'}
              type={props.amountUnit === 'sats' ? 'number' : 'text'}
              min={props.amountUnit === 'sats' ? 1 : undefined}
              step={props.amountUnit === 'sats' ? 1 : undefined}
              onChange={(event) => props.setAmountInput(event.target.value)}
              placeholder={props.amountUnit === 'sats' ? '100000' : '0.001000'}
            />
            <Select value={props.amountUnit} onChange={(event) => props.setAmountUnit(event.target.value as AmountUnit)}>
              <option value="sats">sats</option>
              <option value="btc">BTC</option>
            </Select>
          </div>
        </Field>
        <Field label="Note">
          <Input value={props.note} onChange={(event) => props.setNote(event.target.value)} placeholder="Invoice, contact, memo" />
        </Field>
      </div>
      <div className="rounded-2xl border border-slate-800 bg-slate-950/50 p-3">
        <div className="mb-2 text-xs font-medium uppercase tracking-wide text-slate-500">Fee rate</div>
        <div className="grid gap-2 sm:grid-cols-3">
          <Button className={props.feeMode === 'slow' ? 'border-indigo-500 bg-indigo-950 text-indigo-100' : ''} onClick={() => props.setFeeMode('slow')}>Slow · {props.feeRates.slow} sat/vB</Button>
          <Button className={props.feeMode === 'fast' ? 'border-indigo-500 bg-indigo-950 text-indigo-100' : ''} onClick={() => props.setFeeMode('fast')}>Fast · {props.feeRates.fast} sat/vB</Button>
          <Button className={props.feeMode === 'custom' ? 'border-indigo-500 bg-indigo-950 text-indigo-100' : ''} onClick={() => props.setFeeMode('custom')}>Custom</Button>
        </div>
        {props.feeMode === 'custom' ? (
          <div className="mt-3 max-w-xs"><Input type="number" min={1} value={props.customFeeRate} onChange={(event) => props.setCustomFeeRate(Number(event.target.value))} /></div>
        ) : null}
      </div>
      <Field label="Spend from label" hint="Outputs are grouped by their stable wallet label id.">
        <Select value={String(props.spendFromLabelId)} onChange={(event) => props.setSpendFromLabelId(event.target.value === 'all' ? 'all' : Number(event.target.value))}>
          <option value="all">All labels</option>
          {props.spendableGroups.map((group) => <option key={group.labelId} value={group.labelId}>{group.name} · {formatSats(group.totalSat)} · {group.count} output{group.count === 1 ? '' : 's'}</option>)}
        </Select>
      </Field>
      <div className="grid gap-2 sm:grid-cols-2">
        {props.spendableGroups.map((group) => (
          <button
            key={group.labelId}
            type="button"
            onClick={() => props.setSpendFromLabelId(group.labelId)}
            className={`rounded-xl border p-3 text-left text-sm transition ${props.spendFromLabelId === group.labelId ? 'border-indigo-500 bg-indigo-950/50' : 'border-slate-800 bg-slate-950/40 hover:border-slate-600'}`}
          >
            <div className="font-medium text-slate-100">{group.name}</div>
            <div className="mt-1 text-xs text-slate-500">{group.count} output{group.count === 1 ? '' : 's'} · {formatSats(group.totalSat)}</div>
          </button>
        ))}
      </div>
    </div>
  );
}


function amountInputToBtcString(value: string, unit: AmountUnit): string {
  const clean = value.trim();
  if (!clean) return '';
  if (unit === 'sats') {
    if (!/^\d+$/.test(clean)) throw new Error('Satoshi amount must be a whole number.');
    const sats = Number(clean);
    if (!Number.isSafeInteger(sats) || sats <= 0) throw new Error('Satoshi amount must be greater than zero.');
    return formatBtcFromSats(sats).replace(/ BTC$/, '');
  }
  if (!/^\d+(\.\d{1,6})?$/.test(clean)) {
    throw new Error('BTC amount must use at most 6 decimal places here. Switch to sats for exact satoshi amounts.');
  }
  return clean;
}

function btcSixToSats(value: string): number {
  if (!/^\d+(\.\d{1,6})?$/.test(value.trim())) {
    throw new Error('BTC amount must use at most 6 decimal places.');
  }
  const [whole, fraction = ''] = value.trim().split('.');
  const sats = Number(whole) * 100_000_000 + Number(fraction.padEnd(8, '0'));
  if (!Number.isSafeInteger(sats) || sats <= 0) throw new Error('BTC amount must be greater than zero.');
  return sats;
}

function formatBtcSixFromSats(sats: number): string {
  if (!Number.isSafeInteger(sats) || sats < 0) return '';
  const value = (sats / 100_000_000).toFixed(6);
  return value.replace(/\.?0+$/, '') || '0';
}

interface SpendableGroup {
  labelId: number;
  name: string;
  totalSat: number;
  count: number;
}

function groupUtxosByLabel(utxos: WalletSpendableUtxo[], labels: Array<{ id: number; path: string[] }>): SpendableGroup[] {
  const labelMap = new Map(labels.map((label) => [label.id, label.id === 1 ? '/change' : `/${label.path.join('/')}`]));
  const groups = new Map<number, SpendableGroup>();
  for (const utxo of utxos.filter(isOpenSpendableUtxo)) {
    const current = groups.get(utxo.labelId) ?? { labelId: utxo.labelId, name: labelMap.get(utxo.labelId) ?? `/label-${utxo.labelId}`, totalSat: 0, count: 0 };
    current.totalSat += utxo.valueSat;
    current.count += 1;
    groups.set(utxo.labelId, current);
  }
  return [...groups.values()].sort((a, b) => a.labelId - b.labelId);
}

function totalUtxoValue(utxos: WalletSpendableUtxo[]): number {
  return utxos.filter(isOpenSpendableUtxo).reduce((sum, utxo) => sum + utxo.valueSat, 0);
}

function Info({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="text-xs uppercase tracking-wide text-slate-500">{label}</div>
      <div className="mt-1 break-all text-sm text-slate-200">{value}</div>
    </div>
  );
}

function messageClass(tone: 'success' | 'error' | 'info'): string {
  if (tone === 'success') return 'mt-4 rounded-xl border border-emerald-800 bg-emerald-950/40 p-3 text-sm text-emerald-100';
  if (tone === 'error') return 'mt-4 rounded-xl border border-rose-800 bg-rose-950/40 p-3 text-sm text-rose-100';
  return 'mt-4 rounded-xl border border-sky-800 bg-sky-950/40 p-3 text-sm text-sky-100';
}
