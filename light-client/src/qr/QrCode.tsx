import { useEffect, useState } from 'react';
import QRCode from 'qrcode';
import { EmptyState } from '../components/ui';

export function QrCode({ value, title }: { value: string; title?: string }) {
  const [dataUrl, setDataUrl] = useState<string>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    let active = true;
    setError(undefined);
    setDataUrl(undefined);
    QRCode.toDataURL(value, { errorCorrectionLevel: 'M', margin: 2, width: 320 })
      .then((url) => {
        if (active) setDataUrl(url);
      })
      .catch((err: unknown) => {
        if (active) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      active = false;
    };
  }, [value]);

  if (error) return <EmptyState>Could not render QR code: {error}</EmptyState>;
  if (!dataUrl) return <EmptyState>Rendering QR code…</EmptyState>;

  return (
    <div className="inline-flex flex-col items-center gap-3 rounded-2xl border border-slate-800 bg-white p-4 text-slate-950">
      <img src={dataUrl} alt={title ?? 'QR code'} width={320} height={320} className="h-72 w-72 rounded-xl" />
      {title ? <div className="max-w-72 break-all text-center text-xs text-slate-600">{title}</div> : null}
    </div>
  );
}
