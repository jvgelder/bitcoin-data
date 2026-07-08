import { useEffect, useRef, useState } from 'react';
import { Button, Field, Textarea } from '../components/ui';

declare global {
  interface Window {
    BarcodeDetector?: new (options?: { formats?: string[] }) => BarcodeDetectorLike;
  }
}

interface BarcodeDetectorLike {
  detect(source: CanvasImageSource): Promise<Array<{ rawValue?: string }>>;
}

export function QrScanner({ onScan }: { onScan(value: string): void }) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const [active, setActive] = useState(false);
  const [manual, setManual] = useState('');
  const [error, setError] = useState<string>();

  useEffect(() => {
    if (!active) return undefined;
    let cancelled = false;
    let timeout: number | undefined;

    async function start() {
      try {
        if (!window.BarcodeDetector) {
          setError('This browser does not expose BarcodeDetector. Paste the scanned QR text instead.');
          return;
        }
        const detector = new window.BarcodeDetector({ formats: ['qr_code'] });
        const stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' }, audio: false });
        streamRef.current = stream;
        if (videoRef.current) {
          videoRef.current.srcObject = stream;
          await videoRef.current.play();
        }
        const loop = async () => {
          if (cancelled || !videoRef.current) return;
          try {
            const results = await detector.detect(videoRef.current);
            const raw = results[0]?.rawValue;
            if (raw) {
              onScan(raw);
              setActive(false);
              return;
            }
          } catch {
            // Some browsers throw while video metadata is warming up; retry.
          }
          timeout = window.setTimeout(loop, 250);
        };
        void loop();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    }

    void start();
    return () => {
      cancelled = true;
      if (timeout) window.clearTimeout(timeout);
      streamRef.current?.getTracks().forEach((track) => track.stop());
      streamRef.current = null;
    };
  }, [active, onScan]);

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap gap-2">
        <Button type="button" onClick={() => setActive((value) => !value)}>{active ? 'Stop camera' : 'Scan QR'}</Button>
        <Button type="button" onClick={() => { if (manual.trim()) onScan(manual.trim()); }}>Use pasted text</Button>
      </div>
      {active ? <video ref={videoRef} muted playsInline className="aspect-video w-full rounded-2xl border border-slate-800 bg-black object-cover" /> : null}
      {error ? <p className="text-sm text-amber-200">{error}</p> : null}
      <Field label="Paste QR text" hint="Use this fallback for desktop scanners or browsers without QR camera support.">
        <Textarea rows={4} value={manual} onChange={(event) => setManual(event.target.value)} placeholder="bitcoin:bc1...?... or BIP73 HTTPS request URL or signed PSBT" />
      </Field>
    </div>
  );
}
