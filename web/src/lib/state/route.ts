// Hash routes, so a reload or a shared link opens the same thing:
//   #/c/<collection>/r/<request key>   a collection request
//   #/c/<collection>/f/<flow key>      a flow
//   #/c/<collection>/e/<environment>   an environment
//   #/c/<collection>/d/<folder key>    a folder's settings
//   #/c/<collection>                   a collection's settings
//   #/history/<id>   #/compare/<id>/<id>   #/runs/<id>   #/new
export type Route =
  | { kind: "request"; collection: string; key: string }
  | { kind: "flow"; collection: string; key: string }
  | { kind: "environment"; collection: string; name: string }
  | { kind: "collection"; collection: string }
  | { kind: "folder"; collection: string; key: string }
  | { kind: "history"; id: number }
  | { kind: "compare"; a: number; b: number }
  | { kind: "run"; id: number }
  | { kind: "new" }
  | { kind: "home" };

const enc = (key: string) => key.split("/").map(encodeURIComponent).join("/");

export function formatRoute(r: Route): string {
  switch (r.kind) {
    case "request":
      return `#/c/${encodeURIComponent(r.collection)}/r/${enc(r.key)}`;
    case "flow":
      return `#/c/${encodeURIComponent(r.collection)}/f/${enc(r.key)}`;
    case "environment":
      return `#/c/${encodeURIComponent(r.collection)}/e/${encodeURIComponent(r.name)}`;
    case "collection":
      return `#/c/${encodeURIComponent(r.collection)}`;
    case "folder":
      return `#/c/${encodeURIComponent(r.collection)}/d/${enc(r.key)}`;
    case "history":
      return `#/history/${r.id}`;
    case "compare":
      return `#/compare/${r.a}/${r.b}`;
    case "run":
      return `#/runs/${r.id}`;
    case "new":
      return "#/new";
    case "home":
      return "#/";
  }
}

export function parseRoute(hash: string): Route {
  const parts = hash.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
  const [a, b, c, ...rest] = parts;
  const id = Number(b);
  if (a === "history" && Number.isInteger(id)) return { kind: "history", id };
  if (a === "compare" && Number.isInteger(id) && Number.isInteger(Number(c))) return { kind: "compare", a: id, b: Number(c) };
  if (a === "runs" && Number.isInteger(id)) return { kind: "run", id };
  if (a === "new") return { kind: "new" };
  if (a === "c" && b) {
    const key = rest.join("/");
    if (c === "r" && key) return { kind: "request", collection: b, key };
    if (c === "f" && key) return { kind: "flow", collection: b, key };
    if (c === "e" && key) return { kind: "environment", collection: b, name: key };
    if (c === "d" && key) return { kind: "folder", collection: b, key };
    if (!c) return { kind: "collection", collection: b };
  }
  return { kind: "home" };
}
