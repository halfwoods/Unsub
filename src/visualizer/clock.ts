/** The source of time and of "do this on the next frame", so a test or a benchmark can supply its own. */
export interface Clock {
  /** Milliseconds, monotonic. */
  now(): number;
  /** Calls `callback` with the time, on the next frame at least `delayMs` from now. One request at a time. */
  schedule(callback: (nowMs: number) => void, delayMs: number): void;
  cancel(): void;
}

/** The browser's: animation frames, held back by a timer when the picture is idling. */
export function createBrowserClock(): Clock {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let frame: number | undefined;
  const cancel = () => {
    if (timer !== undefined) clearTimeout(timer);
    if (frame !== undefined) cancelAnimationFrame(frame);
    timer = undefined;
    frame = undefined;
  };
  return {
    now: () => performance.now(),
    schedule(callback, delayMs) {
      cancel();
      if (delayMs > 0) {
        timer = setTimeout(() => {
          timer = undefined;
          frame = requestAnimationFrame(callback);
        }, delayMs);
      } else {
        frame = requestAnimationFrame(callback);
      }
    },
    cancel,
  };
}

/** Time that moves only when told to: for deterministic screenshots and benchmarks. */
export class ManualClock implements Clock {
  private time = 0;
  private pending: ((nowMs: number) => void) | null = null;

  now(): number {
    return this.time;
  }

  schedule(callback: (nowMs: number) => void): void {
    this.pending = callback;
  }

  cancel(): void {
    this.pending = null;
  }

  /** Moves time on by `ms` and runs the frame that was waiting, if any. */
  advance(ms: number): void {
    this.time += ms;
    const callback = this.pending;
    this.pending = null;
    callback?.(this.time);
  }
}
