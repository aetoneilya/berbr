import { div } from "gpui-kit";
import { openOverlay } from "herdr";
import { icon, iconButton } from "./ui.js";

export function initArtifacts(view, cx) {
  view.artifactLists = new Map();
  view.artifactFolders = new Set();
  view.artifactCollapsed = new Set();
  view.artifactHidden = new Set(); // Temporary resource switches must not overwrite the saved choice.
  try {
    const saved = JSON.parse(localStorage.getItem("berbr.artifacts.collapsed"));
    if (Array.isArray(saved)) view.artifactCollapsed = new Set(saved.filter(path => typeof path === "string"));
  } catch (_) { /* Storage is optional. */ }
  // Poll only the expanded task and its visible folders. No recursive scan or
  // filesystem watchers left behind when switching projects/tasks.
  if (!view.form) cx.timer.every(5000, cx => refreshArtifacts(view, cx));
}

function listing(view, task, relative = "") {
  return view.artifactLists.get(task.artifacts)?.get(relative);
}

function requestContext(view, task) {
  const project = task.project ?? view.project;
  return {project, context:{...view.actionContext(project), fallback:view.data?.projects?.find(item => item.name === project)?.cwd}};
}

export function loadArtifacts(view, task, relative, cx, force = false) {
  if (!task?.artifacts || !task.task) return;
  let folders = view.artifactLists.get(task.artifacts);
  if (!folders) view.artifactLists.set(task.artifacts, folders = new Map());
  let state = folders.get(relative);
  if (state?.loading || (!force && state)) return;
  if (!state) folders.set(relative, state = {entries:[], loaded:false});
  const {project, context} = requestContext(view, task);
  state.loading = true;
  cx.notify();
  cx.spawn(async cx => {
    try {
      const data = JSON.parse(await view.cli(["artifacts", task.task, "--list",
        ...(relative ? ["--directory", relative] : [])], {project, context}));
      if (!Array.isArray(data.entries)) throw new Error("Artifact list is unavailable");
      if (data.project !== project || data.task !== task.task) throw new Error("Artifact response belongs to another project or task");
      state.entries = data.entries; state.path = data.path;
      state.error = null; state.loaded = true;
    } catch (error) {
      state.error = String(error.message ?? error);
    }
    state.loading = false; cx.notify();
  });
}

function visibleFolders(view, task, relative = "", result = []) {
  result.push(relative);
  for (const entry of listing(view, task, relative)?.entries ?? []) {
    if (entry.kind === "directory" && view.artifactFolders.has(`${task.artifacts}/${entry.relative}`))
      visibleFolders(view, task, entry.relative, result);
  }
  return result;
}

export function refreshArtifacts(view, cx) {
  if (view.busy || view.form || view.view !== "tasks") return;
  const task = view.sections().filter(section => section.expanded).flatMap(section => section.rows)
    .find(row => row.id === view.expanded && row.kind === "task");
  if (!task?.artifacts || !artifactsExpanded(view, task)) return;
  for (const relative of visibleFolders(view, task)) loadArtifacts(view, task, relative, cx, true);
}

export function artifactsExpanded(view, task) {
  return !view.artifactCollapsed.has(task.artifacts) && !view.artifactHidden.has(task.artifacts);
}

export function setArtifactsExpanded(view, task, expanded, cx, {remember = true} = {}) {
  if (!task.artifacts) return;
  if (!remember && !expanded) { view.artifactHidden.add(task.artifacts); cx.notify(); return; }
  view.artifactHidden.delete(task.artifacts);
  if (expanded) {
    view.history.delete(task.id);
    view.artifactCollapsed.delete(task.artifacts);
    loadArtifacts(view, task, "", cx, true);
  } else view.artifactCollapsed.add(task.artifacts);
  try { localStorage.setItem("berbr.artifacts.collapsed", JSON.stringify([...view.artifactCollapsed])); } catch (_) { /* Storage is optional. */ }
  cx.notify();
}

export function openArtifactFolder(view, task, cx, id, options = {}) {
  const root = listing(view, task), context = requestContext(view, task);
  view.openUrl(root?.path ?? task.artifacts, cx, id, {
    ...options, prepare:root?.loaded ? null : () => view.cli(["artifacts", task.task], context),
  });
}

// Opening a script means editing its text, never running it. Links and special
// files are revealed in Finder rather than followed/executed by an association.
export function artifactOpenArgs(entry) {
  if (entry.kind !== "file") return ["-R", entry.path];
  const script = /\.(sh|bash|zsh|fish|py|pyw|js|mjs|cjs|ts|rb|pl|ps1|bat|cmd|command|scpt|applescript|exe)$/i.test(entry.name);
  return [...(entry.executable || script ? ["-t"] : []), entry.path];
}

function openArtifact(view, entry, cx, id) {
  if (entry.kind === "file" && /\.(md|markdown)$/i.test(entry.name)) {
    openOverlay({mod:"berbr", kind:"window", key:"berbr-artifact", width:720, height:0.75,
      props:{form:"note", path:entry.path, title:entry.name, vault:false, artifact:true}});
  } else view.openUrl(entry.path, cx, id, {args:artifactOpenArgs(entry)});
}

export function artifactItems(view, task) {
  if (!task.artifacts) return [];
  const expanded = artifactsExpanded(view, task);
  const root = listing(view, task);
  const empty = Boolean(root?.loaded && !root.error && !root.entries.length);
  const items = [{type:"artifacts", key:"header", task, expanded, empty, footer:true,
    action:cx => setArtifactsExpanded(view, task, !expanded, cx)}];
  if (!expanded) return items;
  items.push({type:"artifact-tools", key:"tools", task, expanded, empty, resource:true});
  if (empty) return items;
  const visit = (relative, depth) => {
    const state = listing(view, task, relative);
    if (!state?.loaded || state.error || !state.entries.length) {
      items.push({type:"artifact-status", key:`status/${relative}`, depth, resource:true,
        label:state?.error ?? (state?.loaded ? "Empty folder" : "Loading artifacts…"), error:Boolean(state?.error)});
      // Keep the last successful listing visible when a refresh fails.
      if (!state?.error) return;
    }
    for (const entry of state?.entries ?? []) {
      const folder = `${task.artifacts}/${entry.relative}`;
      const expanded = view.artifactFolders.has(folder);
      items.push({type:"artifact", key:entry.relative, entry, expanded, depth, resource:true, action:(cx, id) => {
        if (entry.kind !== "directory") { openArtifact(view, entry, cx, id); return; }
        if (view.artifactFolders.has(folder)) view.artifactFolders.delete(folder);
        else { view.artifactFolders.add(folder); loadArtifacts(view, task, entry.relative, cx, true); }
        cx.notify();
      }});
      if (entry.kind === "directory" && expanded) visit(entry.relative, depth + 1);
    }
  };
  visit("", 0);
  return items;
}

export function renderArtifact(view, t, item, line) {
  if (item.type === "artifact-tools") {
    const task = item.task;
    return line.child(div().flex_1().text_size(11).text_color(t.dim).child(item.empty ? "Empty folder" : "Task files"))
      .child(iconButton(t, `${item.id}/folder`, "folder", "Open artifact folder", cx => {
        openArtifactFolder(view, task, cx, `${item.id}/folder`);
      }, {size:20, disabled:view.busy}))
      .child(iconButton(t, `${item.id}/refresh`, "refresh", "Refresh artifacts", cx => {
        for (const relative of visibleFolders(view, task)) loadArtifacts(view, task, relative, cx, true);
      }, {size:20, disabled:view.busy}));
  }
  line.pl(6 + item.depth * 14);
  if (item.type === "artifact-status") return line.text_size(12).text_color(item.error ? t.blocked : t.dim)
    .tooltip(item.label).child(div().truncate().child(item.label));
  const entry = item.entry, directory = entry.kind === "directory";
  return line.tooltip(entry.path)
    .child(icon(directory ? item.expanded ? "chevron-down" : "chevron-right" : "file-text", 12).text_color(t.dim))
    .when(directory, el => el.child(icon("folder", 12).text_color(t.dim)))
    .child(div().min_w(0).truncate().child(entry.name));
}
