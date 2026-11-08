import { Lock } from "lucide-react";
import { lazy, Suspense, useState } from "react";
import { Button } from "../../components/Button";
import { CopyButton } from "../../components/CopyButton";
import { ErrorPanel } from "../../components/ErrorPanel";
import { LoadingState } from "../../components/LoadingState";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import type { ConfigApplied, ConfigFile, ConfigTarget } from "../../types/dto";
import { SaveConfigDialog } from "./SaveConfigDialog";

const CodeEditor = lazy(() => import("./CodeEditor"));

const normalize = (text: string) => text.replace(/\r\n/g, "\n");

/**
 * The line a tool's error points at in `fileName`, if it names one: nginx `custom.conf:3`,
 * Apache `line 1 of …custom.conf`, PHP `php.ini on line 4`, PostgreSQL `postgresql.conf" line 9`.
 */
export function errorLine(message: string, fileName: string): number | null {
  const name = fileName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const patterns = [
    new RegExp(`${name}:(\\d+)`),
    new RegExp(`line (\\d+) of [^\\n]*?${name}`),
    new RegExp(`${name}"? (?:on )?line (\\d+)`),
  ];
  for (const p of patterns) {
    const m = p.exec(message);
    if (m) return Number(m[1]);
  }
  return null;
}

interface ConfigEditorProps {
  target: ConfigTarget;
  files: ConfigFile[];
  onApplied: (result: ConfigApplied) => void;
  /** Reloads the files from disk (after "changed on disk" or to drop local edits). */
  onReload: () => void;
}

/** The advanced side: the real files, edited as text, checked by the tool on save. */
export function ConfigEditor({ target, files, onApplied, onReload }: ConfigEditorProps) {
  const [key, setKey] = useState(() => (files.find((f) => f.editable) ?? files[0])?.key ?? "");
  const file = files.find((f) => f.key === key) ?? files[0];
  if (!file) return <p className="text-sm text-fg-muted">This service has no configuration files.</p>;
  return (
    <div className="flex flex-col gap-3 text-sm">
      {files.length > 1 && (
        <div role="radiogroup" aria-label="File" className="flex flex-wrap gap-1">
          {files.map((f) => (
            <button
              key={f.key}
              type="button"
              role="radio"
              aria-checked={f.key === file.key}
              onClick={() => setKey(f.key)}
              className={`inline-flex items-center gap-1.5 rounded border px-2.5 py-1 font-mono text-xs ${
                f.key === file.key
                  ? "border-accent bg-accent-subtle font-semibold text-fg"
                  : "border-border text-fg-muted hover:text-fg"
              }`}
            >
              {!f.editable && <Lock aria-hidden className="size-3" />}
              {f.label}
              {!f.editable && <span className="sr-only"> (read-only)</span>}
            </button>
          ))}
        </div>
      )}
      {/* Keyed by file and its hash: a save or reload starts from what's on disk. */}
      <FileEditor
        key={`${file.key}:${file.hash}`}
        target={target}
        file={file}
        onApplied={onApplied}
        onReload={onReload}
      />
    </div>
  );
}

function FileEditor({
  target,
  file,
  onApplied,
  onReload,
}: {
  target: ConfigTarget;
  file: ConfigFile;
  onApplied: (result: ConfigApplied) => void;
  onReload: () => void;
}) {
  const original = normalize(file.text ?? "");
  const [text, setText] = useState(original);
  const [saving, setSaving] = useState(false);
  const [marked, setMarked] = useState<number | null>(null);
  const [backupError, setBackupError] = useState<AppError | null>(null);
  const [loadedBackup, setLoadedBackup] = useState<string | null>(null);
  const readOnly = !file.editable || file.readOnlyReason !== null;
  const dirty = text !== original;

  const loadBackup = async (id: string) => {
    setBackupError(null);
    try {
      setText(normalize(await ipc.readConfigBackup(target, file.key, id)));
      setLoadedBackup(id);
      setMarked(null);
    } catch (e) {
      setBackupError(toAppError(e));
    }
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-col gap-1">
        <p className="flex min-w-0 items-center gap-1 font-mono text-xs break-all text-fg-muted">
          {file.path}
          <CopyButton value={file.path} label={`${file.label} path`} />
        </p>
        <p className="text-pretty text-fg-muted">{file.note}</p>
        {file.editable && file.readOnlyReason && (
          <p className="flex gap-2 rounded-md border border-warning/40 bg-warning-subtle p-2">
            <Lock aria-hidden className="mt-0.5 size-4 shrink-0 text-warning" />
            {file.readOnlyReason}
          </p>
        )}
      </div>

      {file.text === null && !file.editable ? (
        <p className="text-fg-muted">
          This file hasn&apos;t been created yet. Devluma writes it when the service starts.
        </p>
      ) : (
        <div className="h-[min(28rem,55vh)]">
          <Suspense fallback={<LoadingState label="Opening the editor…" shape="panel" rows={8} />}>
            <CodeEditor
              value={text}
              onChange={(v) => {
                setText(v);
                setMarked(null);
              }}
              language={file.language}
              readOnly={readOnly}
              label={`${file.label}${readOnly ? " (read-only)" : ""}`}
              markedLine={marked}
            />
          </Suspense>
        </div>
      )}

      {!readOnly && (
        <div className="flex flex-wrap items-center gap-2">
          <Button variant="primary" disabled={!dirty} onClick={() => setSaving(true)}>
            Review and save
          </Button>
          <Button
            disabled={!dirty}
            onClick={() => {
              setText(original);
              setLoadedBackup(null);
              setMarked(null);
            }}
          >
            Revert
          </Button>
          <Button variant="ghost" onClick={onReload}>
            Reload from disk
          </Button>
          {file.backups.length > 0 && (
            <label className="ml-auto flex items-center gap-2">
              <span className="text-fg-muted">Previous versions</span>
              <select
                className="rounded-md border border-border bg-surface px-2 py-1.5"
                value={loadedBackup ?? ""}
                onChange={(e) => e.target.value && void loadBackup(e.target.value)}
              >
                <option value="">Load a backup…</option>
                {file.backups.map((b) => (
                  <option key={b.id} value={b.id}>
                    {new Date(b.createdAt).toLocaleString()}
                  </option>
                ))}
              </select>
            </label>
          )}
        </div>
      )}
      <p className="text-xs text-fg-muted" aria-live="polite">
        {readOnly
          ? "Read-only."
          : loadedBackup
            ? "A previous version is loaded. Review and save it to restore it; it's checked like any edit."
            : dirty
              ? "Unsaved changes. Ctrl+F searches, Ctrl+Z undoes."
              : "Ctrl+F searches. Saving shows the exact change first, then the tool checks the file."}
      </p>
      {backupError && <ErrorPanel error={backupError} />}

      {saving && (
        <SaveConfigDialog
          target={target}
          change={{ kind: "file", file: file.key, text, baseHash: file.hash }}
          title={`Save ${file.label}?`}
          onClose={() => setSaving(false)}
          onFailed={(message) => setMarked(errorLine(message, file.label))}
          onApplied={onApplied}
        />
      )}
    </div>
  );
}
