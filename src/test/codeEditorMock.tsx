import type { CodeEditorProps } from "../features/config/CodeEditor";

/** Stand-in for the CodeMirror editor, which needs layout APIs jsdom lacks: value in, text out. */
export default function CodeEditorMock(p: CodeEditorProps) {
  return (
    <textarea
      aria-label={p.label}
      data-marked={p.markedLine ?? ""}
      readOnly={p.readOnly}
      value={p.value}
      onChange={(e) => p.onChange(e.target.value)}
    />
  );
}
