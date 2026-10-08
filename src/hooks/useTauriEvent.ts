import { useEffect, useRef } from "react";
import { listen, type Event, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Subscribe to a backend event for the lifetime of the component.
 *
 * `listen()` resolves asynchronously, so a component that unmounts before the
 * promise settles must still unlisten once it does — otherwise every remount
 * (StrictMode, tab switches) stacks another live handler. This hook owns that
 * bookkeeping, and always calls the latest `handler` without re-subscribing.
 */
export function useTauriEvent<T>(
  event: string,
  handler: (payload: T, event: Event<T>) => void,
  enabled = true,
): void {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let unlisten: UnlistenFn | null = null;

    listen<T>(event, (e) => handlerRef.current(e.payload, e))
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((error: unknown) => {
        console.error(`Failed to listen for "${event}":`, error);
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [event, enabled]);
}

/**
 * Collects unlisten functions from async `listen()` calls so they can all be
 * released together, including ones that resolve after `dispose()` ran.
 */
export class ListenerBag {
  private fns: UnlistenFn[] = [];
  private disposed = false;

  add(promise: Promise<UnlistenFn>): void {
    promise
      .then((fn) => {
        if (this.disposed) fn();
        else this.fns.push(fn);
      })
      .catch((error: unknown) => {
        console.error("Failed to register event listener:", error);
      });
  }

  dispose(): void {
    this.disposed = true;
    for (const fn of this.fns) fn();
    this.fns = [];
  }
}
