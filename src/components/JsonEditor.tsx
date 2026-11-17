import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { json } from "@codemirror/lang-json";
import { bracketMatching, HighlightStyle, indentOnInput, syntaxHighlighting } from "@codemirror/language";
import { Annotation, Compartment, EditorState, Facet } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  drawSelection,
  EditorView,
  keymap,
  lineNumbers,
  MatchDecorator,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";
import { tags as t } from "@lezer/highlight";
import { useEffect, useRef } from "react";

const EMPTY: ReadonlySet<string> = new Set();

/** 展開できない変数の名前。変わったら強調表示を作り直す。 */
const unresolvedFacet = Facet.define<ReadonlySet<string>, ReadonlySet<string>>({
  combine: (values) => values[values.length - 1] ?? EMPTY,
});

const varMatcher = new MatchDecorator({
  regexp: /\{\{([^{}\r\n]*)\}\}/g,
  decoration: (match, view) =>
    Decoration.mark({
      class: view.state.facet(unresolvedFacet).has(match[1].trim()) ? "cm-var cm-var-unresolved" : "cm-var",
    }),
});

/** `{{変数}}` を紫（展開できないものは赤）にする。 */
const varPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = varMatcher.createDeco(view);
    }
    update(u: ViewUpdate) {
      this.decorations =
        u.startState.facet(unresolvedFacet) !== u.state.facet(unresolvedFacet)
          ? varMatcher.createDeco(u.view)
          : varMatcher.updateDeco(u, this.decorations);
    }
  },
  { decorations: (v) => v.decorations },
);

const highlight = HighlightStyle.define([
  { tag: t.propertyName, color: "var(--json-key)" },
  { tag: t.string, color: "var(--json-string)" },
  { tag: [t.number, t.bool, t.null], color: "var(--json-number)" },
  { tag: [t.punctuation, t.bracket, t.separator], color: "var(--text-sub)" },
]);

const theme = EditorView.theme(
  {
    "&": { height: "100%", fontSize: "12.5px", backgroundColor: "var(--bg-panel)", color: "var(--text-sub)" },
    "&.cm-focused": { outline: "none" },
    ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.7", overflow: "auto" },
    ".cm-content": { padding: "12px 0", caretColor: "var(--text)" },
    ".cm-line": { padding: "0 14px" },
    ".cm-gutters": {
      backgroundColor: "var(--bg-gutter)",
      color: "var(--text-faint)",
      border: "none",
      borderRight: "1px solid var(--border)",
    },
    ".cm-lineNumbers .cm-gutterElement": { padding: "0 10px" },
    ".cm-cursor": { borderLeftColor: "var(--text)" },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": { backgroundColor: "var(--bg-active)" },
    ".cm-var, .cm-var span": { color: "var(--var) !important" },
    ".cm-var-unresolved, .cm-var-unresolved span": {
      color: "var(--danger) !important",
      textDecoration: "underline wavy var(--danger)",
    },
  },
  { dark: true },
);

/** エディタの外から値を差し替えたときの印（そのときは `onChange` を呼ばない）。 */
const external = Annotation.define<boolean>();

interface Props {
  value: string;
  onChange: (value: string) => void;
  unresolved?: ReadonlySet<string>;
  ariaLabel: string;
}

/** JSON ボディの編集（CodeMirror 6）。行番号・JSON の色分け・`{{変数}}` の強調表示つき。 */
export function JsonEditor({ value, onChange, unresolved, ariaLabel }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const unresolvedSlot = useRef(new Compartment());
  const initial = useRef({ value, unresolved });

  useEffect(() => {
    const view = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: initial.current.value,
        extensions: [
          lineNumbers(),
          history(),
          drawSelection(),
          indentOnInput(),
          bracketMatching(),
          json(),
          syntaxHighlighting(highlight),
          keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
          unresolvedSlot.current.of(unresolvedFacet.of(initial.current.unresolved ?? EMPTY)),
          varPlugin,
          theme,
          EditorView.contentAttributes.of({ "aria-label": ariaLabel, role: "textbox", "aria-multiline": "true" }),
          EditorView.updateListener.of((u) => {
            if (u.docChanged && !u.transactions.some((tr) => tr.annotation(external))) {
              onChangeRef.current(u.state.doc.toString());
            }
          }),
        ],
      }),
    });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, [ariaLabel]);

  useEffect(() => {
    const view = viewRef.current;
    if (view && view.state.doc.toString() !== value) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value }, annotations: external.of(true) });
    }
  }, [value]);

  useEffect(() => {
    viewRef.current?.dispatch({ effects: unresolvedSlot.current.reconfigure(unresolvedFacet.of(unresolved ?? EMPTY)) });
  }, [unresolved]);

  return <div className="json-editor" ref={host} />;
}
