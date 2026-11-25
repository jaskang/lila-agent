import { toast } from "sonner";
import type { DraftImage } from "./types.ts";

type ImageDrafts = Record<string, DraftImage[]>;
type Snapshot = { scope: string; drafts: ImageDrafts };

const SCOPE_KEY = "lila.draftImageScope";
const DATABASE = "lila-composer";
const STORE = "images";
const SNAPSHOT_KEY = "drafts";
let database: Promise<IDBDatabase> | undefined;
let reported = false;

function openDatabase(): Promise<IDBDatabase> {
  return database ??= new Promise((resolve, reject) => {
    const request = indexedDB.open(DATABASE, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
    request.onblocked = () => reject(new Error("Draft storage is blocked"));
  });
}

export async function readDraftImages(): Promise<ImageDrafts> {
  try {
    const scope = sessionStorage.getItem(SCOPE_KEY);
    if (!scope) {
      // A new window session has no matching text drafts.
      await new Promise<void>((resolve) => {
        const request = indexedDB.deleteDatabase(DATABASE);
        request.onsuccess = request.onerror = request.onblocked = () => resolve();
      });
      return {};
    }
    const db = await openDatabase();
    const snapshot = await new Promise<Snapshot | undefined>((resolve, reject) => {
      const request = db.transaction(STORE).objectStore(STORE).get(SNAPSHOT_KEY);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    return snapshot?.scope === scope ? snapshot.drafts : {};
  } catch {
    return {};
  }
}

export async function saveDraftImages(drafts: ImageDrafts): Promise<void> {
  const hasImages = Object.keys(drafts).length > 0;
  try {
    let scope = sessionStorage.getItem(SCOPE_KEY);
    if (!scope) {
      if (!hasImages) return;
      scope = crypto.randomUUID();
      sessionStorage.setItem(SCOPE_KEY, scope);
    }
    const snapshot: Snapshot = { scope, drafts };
    const db = await openDatabase();
    await new Promise<void>((resolve, reject) => {
      const transaction = db.transaction(STORE, "readwrite");
      const store = transaction.objectStore(STORE);
      if (hasImages) store.put(snapshot, SNAPSHOT_KEY);
      else store.delete(SNAPSHOT_KEY);
      transaction.oncomplete = () => resolve();
      transaction.onabort = () => reject(transaction.error);
    });
  } catch {
    if (hasImages && !reported) {
      reported = true;
      toast.warning("图片草稿无法保存，刷新前请先发送或保留原文件");
    }
  }
}
