/**
 * ブラウザ（WebView）の右クリックメニュー（macOS の WebKit だと「再読み込み」「戻る」など）を出さない。
 * デスクトップアプリで再読み込みのメニューが出ると、入力中のタブの状態が消えたように見えて紛らわしいため。
 * 入力欄・テキストエリア・CodeMirror の中でも出さない（コピー・貼り付けはキー操作で行う）。
 *
 * アプリ独自の右クリックメニュー（いまはリクエストのタブの `TabBar`。Radix の ContextMenu）は、この処理と両立する:
 * Radix は自分の `contextmenu` ハンドラ（React のルートで先に届く）で `preventDefault()` してメニューを開き、
 * イベントは止めずに document まで上がるので、ここでもう一度 `preventDefault()` されるだけで害はない。
 * 独自のメニューのない場所では、これまでどおり何も出ない。メニューを持たせたい要素を増やすときも、ここは変えなくてよい。
 */
export function suppressNativeContextMenu(target: Document = document): () => void {
  const onContextMenu = (e: Event) => e.preventDefault();
  target.addEventListener("contextmenu", onContextMenu);
  return () => target.removeEventListener("contextmenu", onContextMenu);
}
