/**
 * ブラウザ（WebView）の右クリックメニュー（macOS の WebKit だと「再読み込み」「戻る」など）を出さない。
 * デスクトップアプリで再読み込みのメニューが出ると、入力中のタブの状態が消えたように見えて紛らわしいため。
 * 入力欄・テキストエリア・CodeMirror の中でも出さない（コピー・貼り付けはキー操作で行う）。
 *
 * 将来アプリ独自の右クリックメニューを作るときは、ここを外す（返す関数で外せる）か、その要素の `contextmenu` で
 * `e.stopPropagation()` する（document まで届かなければこの処理は走らない）。
 */
export function suppressNativeContextMenu(target: Document = document): () => void {
  const onContextMenu = (e: Event) => e.preventDefault();
  target.addEventListener("contextmenu", onContextMenu);
  return () => target.removeEventListener("contextmenu", onContextMenu);
}
