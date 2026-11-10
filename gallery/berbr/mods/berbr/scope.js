export const spaceBindingBlocker = "This Herdr space is shared by tasks in different projects. Resolve the space binding before starting or closing this task.";

// Snapshots stay independently cached. Only presentation IDs are namespaced;
// task IDs, checkout paths, terminal addresses and URLs remain command targets.
export function combineProjects(snapshots, fallback = {}, errors = new Map(), knownSnapshots = snapshots, conflicts = new Map()) {
  const rows = [], catalog = [], warnings = [], stores = {};
  const projects = new Map((fallback?.projects ?? []).map(project => [project.name, project]));
  for (const data of protectSnapshots(snapshots, knownSnapshots, conflicts)) {
    const project = data.project;
    const prefix = `project:${encodeURIComponent(project)}/`;
    const tag = row => ({...row, project, id:row.id == null ? row.id : prefix + row.id,
      ...(row.tasks ? {tasks:row.tasks.map(task => ({...task, project, id:prefix + task.id}))} : {}),
      children:(row.children ?? []).map(tag)});
    rows.push(...(data.rows ?? []).map(tag));
    catalog.push(...(data.merge_requests ?? []).map(tag));
    warnings.push(...(data.warnings ?? []).map(warning => `${project}: ${warning}`));
    for (const item of data.projects ?? []) projects.set(item.name, item);
    stores[project] = data.store;
  }
  for (const [project, error] of errors) warnings.push(`${project}: ${error}`);
  return {...fallback, rows, merge_requests:catalog, warnings, projects:[...projects.values()],
    scope_projects:snapshots.map(data => data.project), project_stores:stores};
}

function protectSnapshots(snapshots, knownSnapshots, conflicts) {
  const spaces = new Map(), bindings = new Map();
  const keyOf = (project, row) => JSON.stringify([project, row.note ?? row.task ?? row.id]);
  const visit = (row, project) => {
    if (row.kind === "task" && row.space) {
      if (!spaces.has(row.space)) spaces.set(row.space, []);
      const binding = {row, project, key:keyOf(project, row)};
      spaces.get(row.space).push(binding); bindings.set(binding.key, binding);
    }
    for (const child of row.children ?? []) visit(child, project);
  };
  for (const data of knownSnapshots) for (const row of data.rows ?? []) visit(row, data.project);
  const ambiguous = new Set();
  for (const [space, owners] of spaces) if (new Set(owners.map(owner => owner.project)).size > 1 &&
      !(owners[0].row.note && owners.every(owner => owner.row.note === owners[0].row.note))) {
    ambiguous.add(space);
    for (const owner of owners) conflicts.set(owner.key, {space, peers:owners.map(owner => owner.key)});
  }
  // Hidden projects remain evidence. Remember conflicts across restarts too;
  // only new, non-conflicting explicit bindings can resolve a known conflict.
  const annotate = (row, project, warnings) => {
    row = {...row, children:(row.children ?? []).map(child => annotate(child, project, warnings))};
    if (row.kind === "task") {
      const key = keyOf(project, row), conflict = conflicts.get(key);
      if (conflict) {
        const resolved = conflict.peers.every(peer => {
          const binding = bindings.get(peer);
          return binding?.row.space && !ambiguous.has(binding.row.space);
        });
        if (resolved || (row.space && row.space !== conflict.space && !ambiguous.has(row.space))) conflicts.delete(key);
        else {
          const mark = child => {child.scope_space_ambiguous = true; (child.children ?? []).forEach(mark);};
          mark(row);
          warnings.push(`Herdr space shared by tasks in different projects: ${project} / ${row.title ?? row.task}. Resolve its binding before starting or closing the task.`);
        }
      }
    }
    return row;
  };
  return snapshots.map(data => {
    const warnings = [...(data.warnings ?? [])];
    return {...data, rows:(data.rows ?? []).map(row => annotate(row, data.project, warnings)), warnings};
  });
}

// Keep ordinary single-project mode protected too, without changing its UI IDs.
export function protectSpaceBindings(data, knownSnapshots, conflicts) {
  return protectSnapshots([data], knownSnapshots, conflicts)[0];
}

// Shared by normal and aggregate refreshes, so changing the selected projects
// cannot start more backends than the limit while earlier calls are in flight.
export class ProjectLoader {
  constructor(fetch, selected = () => true) {
    this.fetch = fetch; this.selected = selected; this.pending = new Map(); this.queue = []; this.active = 0;
  }
  request(project, cached, generation = 0) {
    const key = `${generation}:${cached}:${project}`;
    if (this.pending.has(key)) return this.pending.get(key);
    const request = new Promise((resolve, reject) => this.queue.push({project, cached, generation, key, resolve, reject}));
    this.pending.set(key, request); this.drain();
    return request;
  }
  drain() {
    while (this.active < 2 && this.queue.length) {
      const item = this.queue.shift();
      if (!this.selected(item.project, item.generation)) {
        this.pending.delete(item.key); item.reject(new Error("Project selection changed")); continue;
      }
      this.active++;
      const finish = () => {
        this.active--; this.pending.delete(item.key); this.drain();
      };
      let result;
      try {result = this.fetch(item.project, item.cached);} catch (error) {finish(); item.reject(error); continue;}
      Promise.resolve(result).then(value => {finish(); item.resolve(value);}, error => {finish(); item.reject(error);});
    }
  }
}

export function scopeSummary(selected, projects, current) {
  if (selected === null) return current ?? "Current project";
  if (!selected.length) return "No projects";
  if (selected.length === 1) return selected[0];
  return projects.length && projects.every(project => selected.includes(project.name))
    ? "All projects" : `${selected.length} projects`;
}
