export function walk(rows = []) {
  return (rows ?? []).flatMap(row => [row, ...walk(row.children)]);
}

export function worktreeNameCollision(rows, name) {
  const key = String(name ?? "").trim().toLowerCase();
  return Boolean(key) && walk(rows).some(row => row.kind === "task" && !row.done &&
    [row.task, row.title].some(value => String(value ?? "").trim().toLowerCase() === key));
}

export function cleanCliText(text) {
  return String(text ?? "").replace(/\(eval\):\d+: can't change option: zle/g, "").trim();
}

export function agentActivity(title, app, treeName) {
  let text = String(title ?? "").trim().replace(/^[✳✻✽✶✢✦✧✺•·●○◐◓◑◒⠁-⣿\uFE0F\s]+/u, "").trim();
  const name = String(app ?? "").trim().replace(/[-_]+/g, " ");
  const names = ["claude", "claude code"].includes(name.toLowerCase()) ? ["Claude Code", "Claude"] : [name];
  for (const prefix of names.filter(Boolean)) {
    const start = text.slice(0, prefix.length).replace(/[-_]/g, " ");
    if (start.toLowerCase() === prefix.toLowerCase() &&
        (!text[prefix.length] || /^[\s:—–-]/.test(text.slice(prefix.length)))) {
      text = text.slice(prefix.length).replace(/^[\s:—–-]+/, "").trim();
      break;
    }
  }
  const tree = String(treeName ?? "").trim();
  if (tree) {
    const suffix = tree.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    text = text.replace(new RegExp(`\\s+(?:\\||—|·)\\s+${suffix}\\s*$`, "i"), "").trim();
  }
  return text;
}

export function formatElapsed(ms) {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "";
  const minutes = Math.floor(Math.max(0, ms) / 60000);
  return minutes < 60 ? `${minutes}m` : `${Math.floor(minutes / 60)}h${minutes % 60 ? ` ${minutes % 60}m` : ""}`;
}

export function classifyBlocked(screenText) {
  const lines = String(screenText ?? "").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "")
    .split(/\r?\n/).map(line => line.trim().replace(/^[│┃║❯> ]+|[│┃║ ]+$/g, "").trim()).filter(Boolean);
  const permission = line => /^(?:do you want to (?:proceed|make this edit)|allow\b.*\?|approve(?:\s+(?:command|this command|this action|execution))?\s*[?:]?$|run the following command\b)/i.test(line);
  for (let index = lines.length - 1; index >= 0; index--) {
    if (!permission(lines[index])) continue;
    const previous = lines[index - 1] ?? "";
    const command = /^run the following command\b/i.test(lines[index])
      ? lines[index].replace(/^run the following command\s*:?\s*/i, "") : "";
    const detail = command || (/^(?:[$>]\s*\S|(?:Bash|Edit|Write|Read)\s*\(|(?:git|npm|node|cargo|python\d*|rg|ls|cd)\s|(?:[~/.]|[\w.-]+\/)[^\s]*|[\w.-]+\.[\w]+\b)/i.test(previous)
      ? previous.replace(/^[$>]\s*/, "") : "");
    return detail ? {kind:"permission", detail:detail.slice(0, 80)} : {kind:"permission"};
  }
  const question = [...lines].reverse().find(line => line.endsWith("?") && !permission(line));
  return question ? {kind:"question", detail:question} : {kind:"waiting"};
}

export function closePlan(row, liveSpaces = []) {
  const related = walk(row?.children);
  const unique = items => [...new Map(items.map(item => [item.url || item.cwd || item.id || item.target || item.tree || item, item])).values()];
  const mrs = unique(related.filter(item => item.kind === "mr"));
  const trees = unique(related.filter(item => item.kind === "tree"));
  const removals = trees.map(tree => ({name:tree.tree ?? tree.label ?? "Worktree",
    ...parseTreeMarks(tree.body, tree.state)}));
  const blockers = mrs.filter(mr => ["opened", "open"].includes(mr.state?.toLowerCase()))
    .map(mr => `MR ${mr.label?.match(/!\d+/)?.[0] ?? `!${mr.url?.match(/\/(\d+)\/?$/)?.[1] ?? mr.target ?? "?"}`} is open`);
  blockers.push(...removals.filter(tree => tree.unpushed).map(tree => `${tree.name}: ↑${tree.unpushed} unpushed`));
  // Snapshot tickets have no assignee; the CLI re-checks ownership before closing.
  const jira = [...new Set(related.filter(item => item.kind === "ticket" && !isClosedTicket(item))
    .map(ticket => ticket.target).filter(Boolean))];
  const also = ["Marks the note done"];
  if (row?.space && liveSpaces.some(space => space.id === row.space)) also.push("Closes the Herdr space");
  // Snapshots have no merge timestamps. MR iid is the stable fallback order.
  const merged = mrs.filter(mr => mr.state?.toLowerCase() === "merged").sort((a, b) =>
    Number(b.label?.match(/!(\d+)/)?.[1] ?? 0) - Number(a.label?.match(/!(\d+)/)?.[1] ?? 0));
  return {blockers, removals, jira, also, proof:{count:merged.length, latest:merged[0] ?? null}};
}

export function normalizeTicketState(state) {
  const value = String(state ?? "").trim();
  const normalized = {
    "новый": "New", "new": "New", "open": "New", "to do": "New",
    "в работе": "In progress", "in progress": "In progress",
    "on review": "In review", "in review": "In review", "review": "In review",
    "commited": "Committed", "committed": "Committed",
    "закрыт": "Closed", "closed": "Closed", "done": "Closed", "resolved": "Closed",
  }[value.toLowerCase()];
  return normalized ?? (value.charAt(0).toUpperCase() + value.slice(1).toLowerCase());
}

export function isClosedTicket(row) {
  return ["Closed", "Committed"].includes(normalizeTicketState(row.state));
}

export function parseMrFlags(body = "") {
  const tokens = String(body ?? "").trim().toLowerCase().split(/\s+/);
  const ci = tokens.includes("ci✗") ? "failed"
    : tokens.includes("ci✓") ? "passed" : tokens.includes("ci…") ? "running" : null;
  const conflict = tokens.includes("conflict");
  const draft = tokens.includes("draft");
  const age = tokens.find(token => /^\d+d$/.test(token));
  return {
    ci, conflict, draft, age: age ? Number(age.slice(0, -1)) : null,
    worst: ci === "failed" ? "CI failed" : conflict ? "conflict" : draft ? "draft" : null,
  };
}

export function parseTreeMarks(body = "", state = "") {
  const lines = String(body ?? "").split("\n");
  const first = lines[0].trim();
  const onlyMarks = /^(?:\+\d+|↑\d+|✓\s+merged|quiet\s+\d+d)(?:\s|$)/.test(first);
  const path = onlyMarks ? "" : first;
  const marks = onlyMarks ? lines.join(" ") : lines.slice(1).join(" ");
  const quiet = marks.match(/(?:^|\s)quiet\s+(\d+)d(?:\s|$)/);
  return {
    path,
    changes:Number(marks.match(/(?:^|\s)\+(\d+)(?=\s|$)/)?.[1] ?? 0),
    unpushed:Number(marks.match(/(?:^|\s)↑(\d+)(?=\s|$)/)?.[1] ?? 0),
    merged:/(?:^|\s)✓\s+merged(?:\s|$)/.test(marks) || state?.toLowerCase() === "finished",
    quietDays:quiet ? Number(quiet[1]) : null,
  };
}

export function looseWhy(row) {
  if (row.kind === "ticket") {
    const tree = row.children?.find(child => child.kind === "tree");
    const mr = walk(row.children).find(child => child.kind === "mr" && child.state?.toLowerCase() === "opened");
    const key = row.target ?? "this ticket";
    const needle = String(row.target ?? "").trim().toLowerCase();
    const mentions = text => Boolean(needle) && String(text ?? "").toLowerCase().includes(needle);
    const treeMr = tree?.children?.find(child => child.kind === "mr" && mentions(child.label));
    const mrKey = treeMr?.label?.match(/!\d+/)?.[0];
    const relation = mentions(tree?.branch)
      ? `is on branch ${tree.branch}, which mentions ${key}.`
      : mrKey ? `has MR ${mrKey}, whose title mentions ${key}.` : `is linked to ${key}.`;
    const why = tree
      ? `Your worktree ${tree.label ?? tree.tree ?? "unnamed"} ${relation}`
      : mr ? `Your open MR ${mr.label?.match(/!\d+/)?.[0] ?? ""} mentions ${key}.`
      : "Assigned to you in Jira; nothing started yet.";
    if (!isClosedTicket(row)) return why;
    return tree || mr ? `The ticket is closed, but ${why.charAt(0).toLowerCase()}${why.slice(1)}`
      : "The ticket is closed; no worktree or open MR remains.";
  }
  if (row.kind === "mr") return "Your open MR has no ticket key in its branch or title.";
  if (row.kind === "tree") return "This worktree's branch matches no ticket or task.";
  return "This item is not attached to a task.";
}

export function stripFrontmatter(text) {
  const source = String(text ?? "");
  if (!/^---\r?\n/.test(source)) return source;
  return source.replace(/^---\r?\n[\s\S]*?^---\r?$(?:\n)?/m, "");
}

export function obsidianUri(path) {
  return "obsidian://open?path=" + encodeURIComponent(path);
}

export function cleanObsidian(body = "") {
  return String(body ?? "")
    .split("\n").filter(line => !/^\s*!\[\[[^\]]*\]\]\s*$/.test(line)).join("\n")
    .replace(/!\[\[[^\]]*\]\]/g, "")
    .replace(/\[\[([^\]]+)\]\]/g, (_match, link) => {
      const [target, alias] = link.split("|");
      return alias ?? target.split("#")[0];
    }).trim();
}

export function matchesFilter(row, query) {
  const needle = String(query ?? "").trim().toLowerCase();
  if (!needle) return true;
  return walk([row]).some(item => [item.title, item.label, item.task, item.target,
    item.branch, ...(item.tags ?? [])].filter(Boolean).join(" ").toLowerCase().includes(needle));
}

export function taskDetail(row, liveAgents = []) {
  const children = row?.children ?? [];
  const tickets = children.filter(item => item.kind === "ticket");
  const agentDetail = item => {
    const live = liveAgents.find(agent => item.terminal &&
      (agent.address === item.terminal || agent.pane === item.terminal));
    return { row:item, live, status:live ? live.status : item.state ?? "unknown" };
  };
  const treeRank = tree => tree.agents.some(agent => agent.live) ? 0
    : tree.mr?.state?.toLowerCase() === "opened" ? 1
    : ["merged", "closed"].includes(tree.mr?.state?.toLowerCase()) ? 3 : 2;
  const treeName = tree => tree.row.label ?? tree.row.tree ?? "";
  const trees = children.filter(item => item.kind === "tree").map(tree => ({
    row:tree,
    mr:tree.children?.find(item => item.kind === "mr") ?? null,
    agents:(tree.children ?? []).filter(item => item.kind === "agent").map(agentDetail),
  })).sort((a, b) => treeRank(a) - treeRank(b) || treeName(a).localeCompare(treeName(b)));
  const mrKey = mr => mr.url || mr.id || mr.target || mr.label || mr;
  const attached = new Set(trees.filter(tree => tree.mr).map(tree => mrKey(tree.mr)));
  const loose = new Map();
  const addMr = (mr, ticket) => {
    const key = mrKey(mr);
    if (attached.has(key)) return;
    if (!loose.has(key)) loose.set(key, {row:mr, tickets:[]});
    const tags = loose.get(key).tickets;
    if (ticket && !tags.includes(ticket)) tags.push(ticket);
  };
  for (const child of children) {
    if (child.kind === "mr") addMr(child);
    if (child.kind === "ticket") for (const mr of child.children ?? []) {
      if (mr.kind === "mr") addMr(mr, child.target);
    }
  }
  const paragraph = cleanObsidian(row?.body).split(/\n\s*\n/).map(plainParagraph).find(Boolean) ?? "";
  return {
    tickets:{open:tickets.filter(item => !isClosedTicket(item)), closed:tickets.filter(isClosedTicket)},
    trees,
    mainAgents:children.filter(item => item.kind === "agent").map(agentDetail),
    openMrs:[...loose.values()].filter(item => item.row.state?.toLowerCase() === "opened"),
    history:[...loose.values()].filter(item => ["merged", "closed"].includes(item.row.state?.toLowerCase())),
    subtasks:children.filter(item => item.kind === "task"),
    excerpt:paragraph.length > 240 ? `${paragraph.slice(0, 239).trimEnd()}…` : paragraph,
  };
}

function plainParagraph(text) {
  return text.replace(/^\s{0,3}#{1,6}\s+/gm, "")
    .replace(/^\s*(?:[-+*]|\d+[.)])\s+/gm, "")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/(\*\*|__)(.*?)\1/g, "$2").replace(/\*([^*]+)\*/g, "$1")
    .replace(/(^|\W)_([^_]+)_(?=\W|$)/g, "$1$2").replace(/`+([^`]*?)`+/g, "$1")
    .replace(/\s+/g, " ").trim();
}

export function taskSignals(row, liveAgents = [], liveSpaces = []) {
  const related = walk(row.children);
  const live = new Map(liveAgents.flatMap(agent => [[agent.address, agent], [agent.pane, agent]]));
  const terminals = new Set(related.filter(item => item.kind === "agent").map(item => item.terminal).filter(Boolean));
  const taskAgents = [...new Set([...terminals].map(terminal => live.get(terminal)).filter(Boolean))];
  const working = taskAgents.filter(agent => agent.status === "working").length;
  const attention = taskAgents.some(agent => agent.status === "blocked") ? "blocked"
    : taskAgents.some(agent => agent.status === "done") ? "done" : null;
  const status = row.done ? "closed" : attention ?? (working ? "working" : row.state === "idea" ? "idea" : "open");
  const workingAgent = taskAgents.filter(agent => agent.status === "working")
    .reduce((latest, agent) => !latest || (agent.seq ?? 0) > (latest.seq ?? 0) ? agent : latest, null);
  const seen = new Set();
  const mrs = related.filter(item => {
    if (item.kind !== "mr" || item.state?.toLowerCase() !== "opened" || seen.has(item.id)) return false;
    seen.add(item.id);
    return true;
  });
  return {
    status, working, agents:taskAgents,
    workingAgent,
    workingTree:workingAgent && related.find(item => item.kind === "tree" && walk(item.children).some(child =>
      child.kind === "agent" && child.terminal && [workingAgent.address, workingAgent.pane].includes(child.terminal))),
    attention: attention === "blocked" ? "Needs you" : attention === "done" ? "Finished"
      : working ? `${working} working` : "",
    ticket: related.find(item => item.kind === "ticket" && !isClosedTicket(item) && item.target),
    mrs, mrKey: mrs[0]?.label?.match(/!\d+/)?.[0] ?? "",
    mrFlag: parseMrFlags(mrs[0]?.body).worst,
    trees: related.filter(item => item.kind === "tree"),
    liveSpace: liveSpaces.find(space => space.id === row.space),
  };
}

export function groupSections(rows = [], liveAgents = [], liveSpaces = []) {
  const sections = [
    { id: "needs", label: "Needs you", rows: [] },
    { id: "running", label: "Running", rows: [] },
    { id: "progress", label: "In progress", rows: [] },
    { id: "ideas", label: "Ideas", rows: [] },
    { id: "inbox", label: "Inbox", rows: [], collapsible: true },
    { id: "notInTask", label: "Not in a task", rows: [], collapsible: true },
    { id: "cleanup", label: "Cleanup", rows: [], collapsible: true },
    { id: "done", label: "Done", rows: [] },
  ];
  const byId = new Map(sections.map(section => [section.id, section]));
  const seen = new Set();
  const tasks = (rows ?? []).flatMap(row => row.kind === "header" ? row.children ?? [] : [row]);
  for (const row of tasks) {
    if (row.kind !== "task" || seen.has(row.id)) continue;
    seen.add(row.id);
    const signal = taskSignals(row, liveAgents, liveSpaces);
    const section = row.done ? "done" : ["blocked", "done"].includes(signal.status) ? "needs"
      : signal.working ? "running" : row.state === "idea" ? "ideas" : "progress";
    byId.get(section).rows.push(row);
  }
  let looseSection = "cleanup";
  const looseIds = new Set();
  const addLoose = row => {
    if (row.kind === "task") return;
    if (row.kind === "header") {
      for (const child of row.children ?? []) addLoose(child);
    } else if (!looseIds.has(row.id)) {
      looseIds.add(row.id);
      const section = looseSection === "inbox" ? "inbox"
        : row.kind === "ticket" && !isClosedTicket(row) ? "notInTask" : "cleanup";
      byId.get(section).rows.push(row);
    }
  };
  // Snapshots may nest rows under a header or use it as a sibling delimiter.
  for (const row of rows ?? []) {
    if (row.kind === "header") {
      looseSection = String(row.label ?? "").trim().toLowerCase() === "assigned to me in jira"
        ? "inbox" : "cleanup";
      for (const child of row.children ?? []) addLoose(child);
    } else if (row.kind !== "task") addLoose(row);
  }
  return sections;
}

export function looseReason(row) {
  if (row.kind === "ticket") {
    const tree = (row.children ?? []).find(child => child.kind === "tree" &&
      (!isClosedTicket(row) || child.state?.toLowerCase() === "active"));
    const mrs = [...new Map(walk(row.children)
      .filter(child => child.kind === "mr" && child.state?.toLowerCase() === "opened")
      .map(mr => [mr.url || mr.id || mr, mr])).values()];
    if (!isClosedTicket(row)) return tree ? `worktree ${tree.label ?? tree.tree ?? "unnamed"}`
      : mrs.length ? `MR ${mrs[0].label?.match(/!\d+/)?.[0] ?? mrs[0].target ?? ""} open` : "ticket open";
    const reason = tree ? "ticket closed · worktree left" : "ticket closed";
    return mrs.length ? `${reason} · ${mrs.length} ${mrs.length === 1 ? "MR" : "MRs"} open` : reason;
  }
  const age = row.kind === "mr" ? parseMrFlags(row.body).age : null;
  return age === null ? "no ticket" : `no ticket · ${age}d`;
}

export function visibleSections(sections, { query = "", showDone = false, collapsed = new Set() } = {}) {
  return sections.filter(section => section.id !== "done" || showDone).map(section => {
    const rows = section.rows.filter(row => matchesFilter(row, query));
    return { ...section, rows, expanded: Boolean(query.trim()) || !collapsed.has(section.id) };
  }).filter(section => section.rows.length);
}

export function visibleRows(sections) {
  return sections.flatMap(section => section.expanded ? section.rows : []);
}

export function ticketTitle(row) {
  const label = String(row.label ?? "").trim();
  return row.target && label.startsWith(row.target) ? label.slice(row.target.length).trim() : label;
}

export function rowState(row) {
  if (row.kind === "ticket") return normalizeTicketState(row.state);
  if (row.kind === "mr") return { opened: "Open", merged: "Merged", closed: "Closed" }[row.state?.toLowerCase()] ?? normalizeTicketState(row.state);
  if (row.kind === "tree" && row.state === "active") return "";
  return normalizeTicketState(row.state);
}
