import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import {
  HighlightStyle,
  StreamLanguage,
  bracketMatching,
  syntaxHighlighting,
  type StreamParser,
} from "@codemirror/language";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { EditorState, StateEffect, StateField, type Extension } from "@codemirror/state";
import {
  Decoration,
  EditorView,
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  type DecorationSet,
} from "@codemirror/view";
import { tags as t } from "@lezer/highlight";
import { useEffect, useLayoutEffect, useRef } from "react";

// Lazy-loaded (see ConfigEditor) so CodeMirror stays out of the main bundle.

export interface CodeEditorProps {
  value: string;
  onChange: (value: string) => void;
  /** `ini`, `nginx`, `apache`, `caddyfile`, `yaml` or `text`. */
  language: string;
  readOnly: boolean;
  label: string;
  /** 1-based line to mark, e.g. the one a validator reported. */
  markedLine: number | null;
}

// ---- tiny highlighters ------------------------------------------------------------------------

/** php.ini, my.ini, postgresql.conf: `[section]`, `key = value`, `;`/`#` comments. */
const ini: StreamParser<{ value: boolean }> = {
  startState: () => ({ value: false }),
  token(stream, state) {
    if (stream.sol()) state.value = false;
    if (stream.eatSpace()) return null;
    const ch = stream.peek();
    if (ch === ";" || ch === "#") {
      stream.skipToEnd();
      return "comment";
    }
    if (stream.sol() && ch === "[") {
      stream.skipToEnd();
      return "heading";
    }
    if (ch === '"' || ch === "'") {
      stream.next();
      while (!stream.eol() && stream.next() !== ch);
      return "string";
    }
    if (!state.value) {
      if (stream.match(/^[\w.-]+/)) return "propertyName";
      if (stream.eat("=")) {
        state.value = true;
        return "operator";
      }
    } else if (stream.match(/^-?\d+[A-Za-z]*/)) {
      return "number";
    }
    stream.next();
    return state.value ? "atom" : null;
  },
};

/** nginx / Apache / Caddyfile: `#` comments, a directive word first, `<Tags>`, quoted strings, braces. */
const conf: StreamParser<{ directive: boolean }> = {
  startState: () => ({ directive: true }),
  token(stream, state) {
    if (stream.sol()) state.directive = true;
    if (stream.eatSpace()) return null;
    const ch = stream.peek();
    if (ch === "#") {
      stream.skipToEnd();
      return "comment";
    }
    if (ch === '"' || ch === "'") {
      stream.next();
      while (!stream.eol()) {
        const c = stream.next();
        if (c === "\\") stream.next();
        else if (c === ch) break;
      }
      return "string";
    }
    if (ch === "{" || ch === "}" || ch === ";") {
      stream.next();
      state.directive = true;
      return "punctuation";
    }
    if (stream.match(/^<\/?[A-Za-z]+/)) {
      state.directive = false;
      return "tagName";
    }
    if (stream.match(/^\$[\w{}]+/)) return "variableName";
    if (stream.match(/^\d+[kmgsKMG]?\b/)) return "number";
    if (stream.match(/^[^\s;{}"'#>]+/)) {
      if (state.directive) {
        state.directive = false;
        return "keyword";
      }
      return null;
    }
    stream.next();
    return null;
  },
};

/** mongod.conf (read-only): keys, comments, quoted strings. */
const yaml: StreamParser<null> = {
  startState: () => null,
  token(stream) {
    if (stream.eatSpace()) return null;
    if (stream.peek() === "#") {
      stream.skipToEnd();
      return "comment";
    }
    if (stream.match(/^[\w.]+(?=:)/)) return "propertyName";
    if (stream.match(/^'(?:[^']|'')*'/)) return "string";
    if (stream.match(/^-?\d+(\.\d+)?\b/)) return "number";
    stream.next();
    return null;
  },
};

function languageFor(name: string): Extension {
  switch (name) {
    case "ini":
      return StreamLanguage.define(ini);
    case "nginx":
    case "apache":
    case "caddyfile":
      return StreamLanguage.define(conf);
    case "yaml":
      return StreamLanguage.define(yaml);
    default:
      return [];
  }
}

// ---- theme: the app's own tokens, so both themes follow Windows -------------------------------

const theme = EditorView.theme({
  "&": {
    color: "var(--dl-fg)",
    backgroundColor: "var(--dl-surface-sunken)",
    border: "1px solid var(--dl-border)",
    borderRadius: "6px",
    fontSize: "0.8125rem",
    height: "100%",
  },
  "&.cm-focused": { outline: "2px solid var(--dl-focus)", outlineOffset: "2px" },
  ".cm-scroller": { fontFamily: "var(--font-mono, ui-monospace, monospace)", lineHeight: "1.55" },
  ".cm-content": { caretColor: "var(--dl-accent)", padding: "6px 0" },
  ".cm-cursor": { borderLeftColor: "var(--dl-accent)" },
  ".cm-gutters": {
    backgroundColor: "var(--dl-surface-raised)",
    color: "var(--dl-fg-muted)",
    borderRight: "1px solid var(--dl-border)",
    borderRadius: "6px 0 0 6px",
  },
  ".cm-activeLine": { backgroundColor: "color-mix(in srgb, var(--dl-accent-subtle) 45%, transparent)" },
  ".cm-activeLineGutter": { backgroundColor: "var(--dl-accent-subtle)", color: "var(--dl-fg)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "color-mix(in srgb, var(--dl-accent) 30%, transparent) !important",
  },
  ".cm-selectionMatch": { backgroundColor: "color-mix(in srgb, var(--dl-marker) 25%, transparent)" },
  ".cm-searchMatch": { outline: "1px solid var(--dl-marker)" },
  ".cm-panels": {
    backgroundColor: "var(--dl-surface-raised)",
    color: "var(--dl-fg)",
    borderTop: "1px solid var(--dl-border)",
  },
  ".cm-panels input, .cm-panels button": { color: "var(--dl-fg)" },
  ".cm-textfield": {
    backgroundColor: "var(--dl-surface)",
    border: "1px solid var(--dl-border)",
    borderRadius: "4px",
  },
  ".cm-button": {
    backgroundImage: "none",
    backgroundColor: "var(--dl-surface-raised)",
    border: "1px solid var(--dl-border-strong)",
    borderRadius: "4px",
  },
  ".cm-marked-line": {
    backgroundColor: "var(--dl-danger-subtle)",
    boxShadow: "inset 3px 0 0 var(--dl-danger)",
  },
});

const highlight = HighlightStyle.define([
  { tag: t.comment, color: "var(--dl-fg-muted)", fontStyle: "italic" },
  { tag: [t.heading, t.keyword, t.tagName], color: "var(--dl-accent)", fontWeight: "600" },
  { tag: t.propertyName, color: "var(--dl-fg)", fontWeight: "500" },
  { tag: [t.string, t.atom], color: "var(--dl-success)" },
  { tag: t.number, color: "var(--dl-warning)" },
  { tag: t.variableName, color: "var(--dl-danger)" },
  { tag: [t.operator, t.punctuation], color: "var(--dl-fg-muted)" },
]);

// ---- marked line ------------------------------------------------------------------------------

const setMarked = StateEffect.define<number | null>();
const markedField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    for (const e of tr.effects) {
      if (e.is(setMarked)) {
        const n = e.value;
        if (n === null || n < 1 || n > tr.state.doc.lines) return Decoration.none;
        const line = tr.state.doc.line(n);
        return Decoration.set([Decoration.line({ class: "cm-marked-line" }).range(line.from)]);
      }
    }
    return deco.map(tr.changes);
  },
  provide: (f) => EditorView.decorations.from(f),
});

export default function CodeEditor({
  value,
  onChange,
  language,
  readOnly,
  label,
  markedLine,
}: CodeEditorProps) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  useLayoutEffect(() => {
    onChangeRef.current = onChange;
  });

  // One editor per file/language/mode; value changes from outside are pushed in below.
  useEffect(() => {
    if (!host.current) return;
    const v = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: value,
        extensions: [
          lineNumbers(),
          highlightActiveLineGutter(),
          history(),
          drawSelection(),
          highlightActiveLine(),
          bracketMatching(),
          search({ top: true }),
          highlightSelectionMatches(),
          keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
          languageFor(language),
          syntaxHighlighting(highlight),
          theme,
          markedField,
          EditorState.readOnly.of(readOnly),
          EditorView.editable.of(!readOnly),
          EditorView.contentAttributes.of({
            "aria-label": label,
            "aria-multiline": "true",
            spellcheck: "false",
          }),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) onChangeRef.current(u.state.doc.toString());
          }),
        ],
      }),
    });
    view.current = v;
    return () => {
      v.destroy();
      view.current = null;
    };
    // `value` seeds the document only; later changes are synced by the next effect.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [language, readOnly, label]);

  useEffect(() => {
    const v = view.current;
    if (v && v.state.doc.toString() !== value) {
      v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: value } });
    }
  }, [value]);

  useEffect(() => {
    const v = view.current;
    if (!v) return;
    const effects: StateEffect<unknown>[] = [setMarked.of(markedLine)];
    if (markedLine !== null && markedLine >= 1 && markedLine <= v.state.doc.lines) {
      effects.push(EditorView.scrollIntoView(v.state.doc.line(markedLine).from, { y: "center" }));
    }
    v.dispatch({ effects });
  }, [markedLine, value]);

  return <div ref={host} className="h-full min-h-0" />;
}
