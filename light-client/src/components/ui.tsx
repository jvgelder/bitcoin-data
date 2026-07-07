import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode, SelectHTMLAttributes, TextareaHTMLAttributes } from 'react';

export function Card({ title, subtitle, children, className = '' }: { title?: string; subtitle?: string; children: ReactNode; className?: string }) {
  return (
    <section className={`rounded-2xl border border-slate-800 bg-slate-900/75 p-5 shadow-xl shadow-slate-950/20 ${className}`}>
      {(title || subtitle) && (
        <div className="mb-4">
          {title && <h2 className="text-base font-semibold text-slate-50">{title}</h2>}
          {subtitle && <p className="mt-1 text-sm text-slate-400">{subtitle}</p>}
        </div>
      )}
      {children}
    </section>
  );
}

export function Button({ className = '', ...props }: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      {...props}
      className={`inline-flex items-center justify-center rounded-xl border border-slate-700 bg-slate-800 px-4 py-2 text-sm font-medium text-slate-100 transition hover:border-slate-500 hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-50 ${className}`}
    />
  );
}

export function PrimaryButton({ className = '', ...props }: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <Button
      {...props}
      className={`border-indigo-500 bg-indigo-500 text-white hover:border-indigo-400 hover:bg-indigo-400 ${className}`}
    />
  );
}

export function DangerButton({ className = '', ...props }: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <Button
      {...props}
      className={`border-rose-700 bg-rose-950/80 text-rose-100 hover:border-rose-500 hover:bg-rose-900 ${className}`}
    />
  );
}

export function GhostButton({ className = '', ...props }: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      {...props}
      className={`inline-flex items-center justify-center rounded-xl px-3 py-2 text-sm font-medium text-slate-300 transition hover:bg-slate-800 hover:text-white disabled:cursor-not-allowed disabled:opacity-50 ${className}`}
    />
  );
}

export function Input({ className = '', ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      {...props}
      className={`w-full rounded-xl border border-slate-700 bg-slate-950/70 px-3 py-2 text-sm text-slate-100 outline-none transition placeholder:text-slate-600 focus:border-indigo-400 focus:ring-2 focus:ring-indigo-400/20 ${className}`}
    />
  );
}

export function Textarea({ className = '', ...props }: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      {...props}
      className={`w-full rounded-xl border border-slate-700 bg-slate-950/70 px-3 py-2 text-sm text-slate-100 outline-none transition placeholder:text-slate-600 focus:border-indigo-400 focus:ring-2 focus:ring-indigo-400/20 ${className}`}
    />
  );
}

export function Select({ className = '', ...props }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select
      {...props}
      className={`w-full rounded-xl border border-slate-700 bg-slate-950/70 px-3 py-2 text-sm text-slate-100 outline-none transition focus:border-indigo-400 focus:ring-2 focus:ring-indigo-400/20 ${className}`}
    />
  );
}

export function Field({ label, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return (
    <label className="block">
      <span className="mb-1 block text-xs font-medium uppercase tracking-wide text-slate-500">{label}</span>
      {children}
      {hint && <span className="mt-1 block text-xs text-slate-500">{hint}</span>}
    </label>
  );
}

export function Badge({ tone = 'slate', children }: { tone?: 'slate' | 'green' | 'yellow' | 'red' | 'indigo' | 'blue'; children: ReactNode }) {
  const tones = {
    slate: 'border-slate-700 bg-slate-800 text-slate-300',
    green: 'border-emerald-700/70 bg-emerald-950/80 text-emerald-200',
    yellow: 'border-amber-700/70 bg-amber-950/80 text-amber-200',
    red: 'border-rose-700/70 bg-rose-950/80 text-rose-200',
    indigo: 'border-indigo-700/70 bg-indigo-950/80 text-indigo-200',
    blue: 'border-sky-700/70 bg-sky-950/80 text-sky-200',
  };
  return <span className={`inline-flex rounded-full border px-2.5 py-1 text-xs font-medium ${tones[tone]}`}>{children}</span>;
}

export function Toggle({ checked, onChange, label, tooltip }: { checked: boolean; onChange(next: boolean): void; label: string; tooltip?: string }) {
  return (
    <button
      type="button"
      onClick={() => onChange(!checked)}
      title={tooltip}
      className="flex w-full items-center justify-between gap-3 rounded-xl border border-slate-800 bg-slate-950/50 px-3 py-2 text-left text-sm text-slate-200"
    >
      <span>
        {label}
        {tooltip ? <span className="ml-2 rounded-full border border-slate-700 px-1.5 text-[10px] text-slate-500">?</span> : null}
      </span>
      <span
        className={`relative inline-flex h-6 w-11 shrink-0 rounded-full transition ${checked ? 'bg-indigo-500' : 'bg-slate-700'}`}
      >
        <span
          className={`absolute top-1 h-4 w-4 rounded-full bg-white transition ${checked ? 'left-6' : 'left-1'}`}
        />
      </span>
    </button>
  );
}

export function EmptyState({ children }: { children: ReactNode }) {
  return <div className="rounded-xl border border-dashed border-slate-800 p-8 text-center text-sm text-slate-500">{children}</div>;
}
