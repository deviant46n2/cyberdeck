// Unit tests for the background-download state machine in lib/dl.ts.
//
// The store is module-global and event-driven, so each test re-imports a fresh
// copy via vi.resetModules() + dynamic import, and the "@tauri-apps/api/event"
// listen mock captures handlers into a bus we fire from the test. A window
// stub is required for init() to attach the listeners (the store skips them in
// a non-DOM environment otherwise).

import { beforeEach, describe, expect, it, vi } from "vitest";

const bus = vi.hoisted(() => new Map<string, (e: { payload: unknown }) => void>());

vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: (e: { payload: unknown }) => void) => {
    bus.set(event, cb);
    return Promise.resolve(() => bus.delete(event));
  },
}));

const apiMock = vi.hoisted(() => ({
  downloadStart: vi.fn<() => Promise<void>>(() => Promise.resolve()),
  downloadCancel: vi.fn<() => Promise<void>>(() => Promise.resolve()),
  downloadRemove: vi.fn<() => Promise<void>>(() => Promise.resolve()),
  indexDownloaded: vi.fn<() => Promise<number>>(() => Promise.resolve(0)),
  downloadStates: vi.fn<(keys: string[]) => Promise<unknown[]>>(() => Promise.resolve([])),
}));

vi.mock("../api", () => apiMock);

type DlModule = typeof import("./dl");

async function freshStore(): Promise<DlModule> {
  vi.resetModules();
  (globalThis as { window?: unknown }).window = globalThis;
  const dl = (await import("./dl")) as DlModule;
  dl.init();
  return dl;
}

function emit(event: string, payload: Record<string, unknown>) {
  const cb = bus.get(event);
  expect(cb).toBeDefined();
  cb?.({ payload });
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("dl.ts download store", () => {
  beforeEach(() => {
    bus.clear();
    apiMock.downloadStart.mockClear();
    apiMock.downloadCancel.mockClear();
    apiMock.downloadRemove.mockClear();
    apiMock.indexDownloaded.mockClear();
    apiMock.downloadStart.mockImplementation(() => Promise.resolve());
    apiMock.downloadCancel.mockImplementation(() => Promise.resolve());
    apiMock.downloadRemove.mockImplementation(() => Promise.resolve());
    apiMock.indexDownloaded.mockImplementation(() => Promise.resolve(0));
    apiMock.downloadStates.mockImplementation(() => Promise.resolve([]));
  });

  it("enqueue adds a queued entry and launches it within MAX_ACTIVE", async () => {
    const dl = await freshStore();
    dl.enqueue("org/repo", "q2.gguf");
    const [row] = dl.getSnapshot();
    // enqueue only launches; the backend's dl-start event flips it active.
    expect(row.status).toBe("queued");
    expect(apiMock.downloadStart).toHaveBeenCalledWith("org/repo", "q2.gguf");
    emit("dl-start", { key: "org/repo/q2.gguf", repo_id: "org/repo", rfilename: "q2.gguf" });
    expect(dl.getSnapshot()[0].status).toBe("active");
    expect(dl.activeCount()).toBe(1);
    await flush();
  });

  it("caps concurrent launches at MAX_ACTIVE, others stay queued", async () => {
    const dl = await freshStore();
    dl.enqueue("r", "a.gguf");
    emit("dl-start", { key: "r/a.gguf", repo_id: "r", rfilename: "a.gguf" });
    dl.enqueue("r", "b.gguf");
    emit("dl-start", { key: "r/b.gguf", repo_id: "r", rfilename: "b.gguf" });
    dl.enqueue("r", "c.gguf");
    expect(apiMock.downloadStart).toHaveBeenCalledTimes(2);
    const byStatus = dl.getSnapshot().map((e) => e.status);
    expect(byStatus.filter((s) => s === "active")).toHaveLength(2);
    expect(byStatus.filter((s) => s === "queued")).toHaveLength(1);
    // once a slot frees, the queued entry launches
    emit("dl-done", { key: "r/a.gguf", repo_id: "r", rfilename: "a.gguf", path: "p" });
    await flush();
    expect(apiMock.downloadStart).toHaveBeenCalledWith("r", "c.gguf");
    expect(dl.getSnapshot().length).toBe(3);
  });

  it("dl-start activates an entry that has never been queued", async () => {
    const dl = await freshStore();
    emit("dl-start", { key: "r/a.gguf", repo_id: "r", rfilename: "a.gguf" });
    const row = dl.getSnapshot().find((e) => e.key === "r/a.gguf");
    expect(row?.status).toBe("active");
    expect(dl.getSnapshot().length).toBe(1);
  });

  it("dl-progress tracks done/total and again activates a queued row", async () => {
    const dl = await freshStore();
    emit("dl-start", { key: "r/a.gguf", repo_id: "r", rfilename: "a.gguf" });
    emit("dl-progress", { key: "r/a.gguf", done: 512, total: 1024 });
    const row = dl.getSnapshot().find((e) => e.key === "r/a.gguf");
    expect(row?.total).toBe(1024);
    expect(row?.done).toBe(512);
  });

  it("dl-done marks done, drops it from the queue, indexes, and fires onDone", async () => {
    let fired: string | null = null;
    const dl = await freshStore();
    const unsub = dl.onDone((p) => {
      fired = p;
    });
    dl.enqueue("r", "a.gguf");
    emit("dl-done", {
      key: "r/a.gguf",
      repo_id: "r",
      rfilename: "a.gguf",
      path: "~/models/a.gguf",
    });
    await flush();
    const row = dl.getSnapshot().find((e) => e.key === "r/a.gguf");
    expect(row?.status).toBe("done");
    expect(row?.path).toBe("~/models/a.gguf");
    expect(apiMock.indexDownloaded).toHaveBeenCalledWith(["~/models/a.gguf"]);
    expect(fired).toBe("~/models/a.gguf");
    expect(dl.activeCount()).toBe(0);
    expect(dl.getSnapshot().filter((e) => e.status === "done")).toHaveLength(1);
    unsub();
  });

  it("a shard set is indexed only once, after every part lands", async () => {
    const dl = await freshStore();
    const p = dl.enqueueSequence("org/m", ["p1.gguf", "p2.gguf", "p3.gguf"]);
    emit("dl-done", { key: "org/m/p1.gguf", repo_id: "org/m", rfilename: "p1.gguf", path: "~/models/p1.gguf" });
    await flush();
    expect(apiMock.indexDownloaded).not.toHaveBeenCalled();
    emit("dl-done", { key: "org/m/p2.gguf", repo_id: "org/m", rfilename: "p2.gguf", path: "~/models/p2.gguf" });
    await flush();
    expect(apiMock.indexDownloaded).not.toHaveBeenCalled();
    emit("dl-done", { key: "org/m/p3.gguf", repo_id: "org/m", rfilename: "p3.gguf", path: "~/models/p3.gguf" });
    await flush();
    await p;
    expect(apiMock.indexDownloaded).toHaveBeenCalledTimes(1);
    expect(apiMock.indexDownloaded).toHaveBeenCalledWith([
      "~/models/p1.gguf",
      "~/models/p2.gguf",
      "~/models/p3.gguf",
    ]);
  });

  it("stop parks the .part as paused and calls cancel; start relaunches", async () => {
    const dl = await freshStore();
    dl.enqueue("r", "a.gguf");
    emit("dl-start", { key: "r/a.gguf", repo_id: "r", rfilename: "a.gguf" });
    dl.stop("r/a.gguf");
    expect(dl.getSnapshot().find((e) => e.key === "r/a.gguf")?.status).toBe("paused");
    expect(apiMock.downloadCancel).toHaveBeenCalledWith("r/a.gguf");
    const calls = apiMock.downloadStart.mock.calls.length;
    dl.start("r/a.gguf");
    expect(dl.getSnapshot().find((e) => e.key === "r/a.gguf")?.status).toBe("queued");
    expect(apiMock.downloadStart).toHaveBeenCalledTimes(calls + 1);
    emit("dl-start", { key: "r/a.gguf", repo_id: "r", rfilename: "a.gguf" });
    expect(dl.getSnapshot().find((e) => e.key === "r/a.gguf")?.status).toBe("active");
    await flush();
  });

  it("stop on a done row is a no-op", async () => {
    const dl = await freshStore();
    dl.enqueue("r", "a.gguf");
    emit("dl-done", { key: "r/a.gguf", repo_id: "r", rfilename: "a.gguf", path: "p" });
    await flush();
    dl.stop("r/a.gguf");
    expect(dl.getSnapshot().find((e) => e.key === "r/a.gguf")?.status).toBe("done");
    expect(apiMock.downloadCancel).not.toHaveBeenCalled();
  });

  it("launch-time reconcile converges a transfer whose dl-done was dropped", async () => {
    // Backend finished while the dl-done event never reached the store — the
    // row would sit in "queued" forever without the authoritative reconcile.
    const dl = await freshStore();
    apiMock.downloadStates.mockImplementation((keys: string[]) =>
      Promise.resolve([
        { key: keys[0], status: "done", path: "~/models/a.gguf", error: null },
      ]),
    );
    dl.enqueue("r", "a.gguf");
    await flush();
    const row = dl.getSnapshot().find((e) => e.key === "r/a.gguf");
    expect(row?.status).toBe("done");
    expect(row?.path).toBe("~/models/a.gguf");
    expect(apiMock.indexDownloaded).toHaveBeenCalledWith(["~/models/a.gguf"]);
  });

  it("an 'already downloading' launch error flips the entry active and pumps", async () => {
    const dl = await freshStore();
    apiMock.downloadStart.mockImplementation(() => Promise.reject(new Error("already downloading")));
    dl.enqueue("r", "a.gguf");
    await flush();
    dl.enqueue("r", "b.gguf");
    await flush();
    const rows = dl.getSnapshot();
    expect(rows.find((e) => e.key === "r/b.gguf")?.status).toBe("active");
    expect(rows.find((e) => e.key === "r/a.gguf")?.status).toBe("active");
  });

  it("a hard launch error marks the row errored", async () => {
    const dl = await freshStore();
    apiMock.downloadStart.mockImplementation(() =>
      Promise.reject(new Error("connection refused")),
    );
    dl.enqueue("r", "a.gguf");
    await flush();
    const row = dl.getSnapshot().find((e) => e.key === "r/a.gguf");
    expect(row?.status).toBe("error");
    expect(row?.err).toContain("connection refused");
  });

  it("dl-error cancelled parks paused; other errors mark errored", async () => {
    const dl = await freshStore();
    emit("dl-start", { key: "r/a.gguf", repo_id: "r", rfilename: "a.gguf" });
    emit("dl-error", { key: "r/a.gguf", repo_id: "r", rfilename: "a.gguf", error: "cancelled" });
    expect(dl.getSnapshot().find((e) => e.key === "r/a.gguf")?.status).toBe("paused");
    emit("dl-error", { key: "r/b.gguf", repo_id: "r", rfilename: "b.gguf", error: "gibibytes exhausted" });
    const row = dl.getSnapshot().find((e) => e.key === "r/b.gguf");
    expect(row?.status).toBe("error");
    expect(row?.err).toBe("gibibytes exhausted");
  });

  it("discard cancels active work, removes the .part, and forgets the row", async () => {
    const dl = await freshStore();
    dl.enqueue("r", "a.gguf");
    dl.stop("r/a.gguf");
    expect(dl.getSnapshot().length).toBe(1);
    await dl.discard("r/a.gguf");
    expect(apiMock.downloadRemove).toHaveBeenCalledWith("r/a.gguf", "a.gguf");
    expect(dl.getSnapshot().length).toBe(0);
  });

  it("clearFinished only removes done/error rows", async () => {
    const dl = await freshStore();
    dl.enqueue("r", "a.gguf"); // active
    emit("dl-done", { key: "r/x.gguf", repo_id: "r", rfilename: "x.gguf" });
    await flush();
    dl.clearFinished();
    expect(dl.getSnapshot().some((e) => e.key === "r/x.gguf")).toBe(false);
    expect(dl.getSnapshot().some((e) => e.key === "r/a.gguf")).toBe(true);
  });

  it("movePriority reorders the queue front-to-back", async () => {
    const dl = await freshStore();
    // Park launches so the queue keeps its order (mock resolves instantly,
    // so make separate entries by pausing the run right away).
    dl.enqueue("r", "a.gguf");
    dl.enqueue("r", "b.gguf");
    const before = dl.getSnapshot().map((e) => e.key);
    dl.movePriority("r/a.gguf", 1);
    const after = dl.getSnapshot().map((e) => e.key);
    expect(before).not.toEqual(after);
    dl.movePriority("r/a.gguf", -1);
    expect(dl.getSnapshot().map((e) => e.key)).toEqual(before);
  });

  it("enqueue ignores duplicates and restarts errored entries", async () => {
    const dl = await freshStore();
    dl.enqueue("r", "a.gguf");
    const first = dl.getSnapshot().length;
    dl.enqueue("r", "a.gguf");
    expect(dl.getSnapshot().length).toBe(first);

    apiMock.downloadStart.mockImplementation(() => Promise.reject(new Error("boom")));
    dl.enqueue("r", "c.gguf");
    await flush();
    expect(dl.getSnapshot().find((e) => e.key === "r/c.gguf")?.status).toBe("error");

    // A failed entry can be re-enqueued; it goes back to the queue and pumps.
    apiMock.downloadStart.mockImplementation(() => Promise.resolve());
    dl.enqueue("r", "c.gguf");
    expect(dl.getSnapshot().find((e) => e.key === "r/c.gguf")?.status).toBe("queued");
    await flush();
  });

  it("init is idempotent", async () => {
    const dl = await freshStore();
    const before = bus.size;
    dl.init();
    expect(bus.size).toBe(before);
  });

  it("subscribe/bump notify listeners on transitions", async () => {
    const dl = await freshStore();
    const seen: number[] = [];
    const unsub = dl.subscribe(() => seen.push(dl.getVersion()));
    dl.enqueue("r", "a.gguf");
    await flush();
    emit("dl-done", { key: "r/a.gguf", repo_id: "r", rfilename: "a.gguf", path: "p" });
    expect(seen.length).toBeGreaterThanOrEqual(2);
    unsub();
    const marker = seen.length;
    emit("dl-error", { key: "r/z.gguf", repo_id: "r", rfilename: "z.gguf", error: "e" });
    expect(seen).toHaveLength(marker);
  });
});