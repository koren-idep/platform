import type { DirectoryEntry } from '../services/protocol';

export interface TreeNode { path: string; expanded: boolean; loading: boolean; entries: DirectoryEntry[]; error: string | null }
export class ProjectTreeState {
  readonly nodes = new Map<string, TreeNode>();
  constructor(readonly root = '') { this.nodes.set(root, { path: root, expanded: true, loading: false, entries: [], error: null }); }
  toggle(path: string): boolean {
    const node = this.nodes.get(path);
    if (!node) return false;
    node.expanded = !node.expanded;
    return node.expanded;
  }
  setLoading(path: string, loading: boolean): void {
    const node = this.nodes.get(path);
    if (node) node.loading = loading;
  }
  setEntries(path: string, entries: DirectoryEntry[]): void {
    const node = this.nodes.get(path);
    if (!node) return;
    node.entries = entries;
    node.loading = false;
    node.error = null;
  }
  setError(path: string, error: string): void {
    const node = this.nodes.get(path);
    if (node) { node.loading = false; node.error = error; }
  }
  child(path: string): TreeNode {
    let node = this.nodes.get(path);
    if (!node) {
      node = { path, expanded: false, loading: false, entries: [], error: null };
      this.nodes.set(path, node);
    }
    return node;
  }
  visible(): Array<{ entry: DirectoryEntry; depth: number }> {
    const out: Array<{ entry: DirectoryEntry; depth: number }> = [];
    const visit = (path: string, depth: number): void => {
      const node = this.nodes.get(path);
      if (!node?.expanded) return;
      for (const entry of node.entries) {
        out.push({ entry, depth });
        if (entry.kind === 'directory') visit(entry.path, depth + 1);
      }
    };
    visit(this.root, 0);
    return out;
  }
}

export interface EditorTab { id: number; path: string; title: string; revision: number; dirty: boolean }
export class TabState {
  readonly tabs: EditorTab[] = [];
  activeId: number | null = null;
  open(tab: EditorTab): void {
    const existing = this.tabs.find((item) => item.id === tab.id);
    if (existing) { existing.path = tab.path; existing.title = tab.title; existing.revision = tab.revision; existing.dirty = tab.dirty; }
    else this.tabs.push(tab);
    this.activeId = tab.id;
  }
  select(id: number): boolean {
    if (!this.tabs.some((tab) => tab.id === id)) return false;
    this.activeId = id;
    return true;
  }
  update(id: number, patch: Partial<Pick<EditorTab, 'revision' | 'dirty'>>): void {
    const tab = this.tabs.find((item) => item.id === id);
    if (tab) Object.assign(tab, patch);
  }
  close(id: number): void {
    const index = this.tabs.findIndex((tab) => tab.id === id);
    if (index < 0) return;
    this.tabs.splice(index, 1);
    if (this.activeId === id) this.activeId = this.tabs[Math.max(0, index - 1)]?.id ?? null;
  }
  get active(): EditorTab | undefined { return this.tabs.find((tab) => tab.id === this.activeId); }
}
