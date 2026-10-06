/** Messages between the sidecar and its Innertube support worker (Node IPC). */
export type WorkerRequest =
  | { id: number; type: 'init'; challenge: unknown }
  | { id: number; type: 'pot'; identifier: string }
  | { id: number; type: 'run'; data: { output: string }; env: Record<string, unknown> };

export type WorkerReply = { id: number; ok: true; result: unknown } | { id: number; ok: false; error: string };

/** Distributive `Omit` that keeps the union's discriminant. */
export type WithoutId<T> = T extends unknown ? Omit<T, 'id'> : never;
