import { div } from "gpui-kit";
import { openOverlay } from "herdr";
import { walk } from "./model.js";
import { icon, iconButton } from "./ui.js";

export function initArtifacts(view, cx) {
  view.artifactLists = new Map();
  view.artifactFolders = new Set();
  // Poll only the expanded task and its visible folders. No recursive scan or
  // filesystem watchers left behind when switching projects/tasks.
  if (!view.form) cx.timer.every(5000, cx => refreshArtifacts(view, cx));
}

function listing(view, task, relative = "") {
  return view.artifactLists.get(task.artifacts)?.get(relative);
}

export function loadArtifacts(view, task, relative, cx, force = false) {
  if (!task?.artifacts || !task.task) return;
  let folders = view.artifactLists.get(task.artifacts);
  if (!folders) view.artifactLists.set(task.artifacts, folders = new Map());
  let state = folders.get(relative);
  if (state?.loading || (!force && state)) return;
  if (!state) folders.set(relative, state = {entries:[], loaded:false});
  const project = view.project;
  const context = {...view.actionContext(), fallback:view.data?.projects?.find(item => item.name === project)?.cwd};
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
  if (view.busy) return;
  const task = walk(view.data?.rows).find(row => row.id === view.expanded && row.kind === "task");
  if (!task?.artifacts) return;
  for (const relative of visibleFolders(view, task)) loadArtifacts(view, task, relative, cx, true);
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
  const items = [{type:"artifacts", key:"header", task, groupStart:true}];
  const visit = (relative, depth) => {
    const state = listing(view, task, relative);
    if (!state?.loaded || state.error || !state.entries.length) {
      items.push({type:"artifact-status", key:`status/${relative}`, depth,
        label:state?.error ?? (state?.loaded ? "Empty folder" : "Loading artifacts…"), error:Boolean(state?.error)});
      // Keep the last successful listing visible when a refresh fails.
      if (!state?.error) return;
    }
    for (const entry of state?.entries ?? []) {
      const folder = `${task.artifacts}/${entry.relative}`;
      const expanded = view.artifactFolders.has(folder);
      items.push({type:"artifact", key:entry.relative, entry, expanded, depth, action:(cx, id) => {
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
  if (item.type === "artifacts") {
    const task = item.task;
    return line.child(icon("folder", 12).text_color(t.dim))
      .child(div().flex_1().text_size(12).text_color(t.muted).child("Artifacts"))
      .child(iconButton(t, `${item.id}/folder`, "folder", "Open artifact folder", cx => {
        if (listing(view, task)?.loaded) view.openUrl(listing(view, task).path, cx);
        else loadArtifacts(view, task, "", cx, true);
      }, {size:20, disabled:view.busy || !listing(view, task)?.loaded}))
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
