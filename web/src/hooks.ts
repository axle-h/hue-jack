import { useEffect, useRef, useState } from 'preact/hooks';

/** Calls `fn` now and every `ms` while mounted (and while `enabled`). */
export function useInterval(fn: () => void, ms: number, enabled = true) {
  const saved = useRef(fn);
  saved.current = fn;
  useEffect(() => {
    if (!enabled) return;
    saved.current();
    const id = setInterval(() => saved.current(), ms);
    return () => clearInterval(id);
  }, [ms, enabled]);
}

/**
 * A locally editable copy of a server value. Edits show immediately and are sent with `commit` after `delayMs`
 * of no further edits; server updates replace the local value only when no edit is pending.
 */
export function useDebouncedValue<T>(
  server: T,
  commit: (v: T) => void,
  delayMs = 150,
): [T, (v: T) => void] {
  const [local, setLocal] = useState(server);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const commitRef = useRef(commit);
  commitRef.current = commit;

  useEffect(() => {
    if (timer.current === null) setLocal(server);
  }, [server]);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  const set = (v: T) => {
    setLocal(v);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      timer.current = null;
      commitRef.current(v);
    }, delayMs);
  };
  return [local, set];
}

/** Counts down from `secs` (re-synced whenever `secs` changes), once per second. */
export function useCountdown(secs: number): number {
  const [left, setLeft] = useState(secs);
  useEffect(() => {
    setLeft(secs);
    if (secs <= 0) return;
    const start = Date.now();
    const id = setInterval(() => {
      const l = Math.max(0, secs - Math.floor((Date.now() - start) / 1000));
      setLeft(l);
      if (l <= 0) clearInterval(id);
    }, 250);
    return () => clearInterval(id);
  }, [secs]);
  return left;
}
