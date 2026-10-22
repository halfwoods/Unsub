/**
 * The motion behind the Lines style: a few polygons whose corners drift across the picture and
 * bounce off its edges, each remembering where it has been so that the picture can draw the last
 * second or so of it as a trail. Coordinates are as the picture has them: y from -1 to 1, x from
 * -aspect to aspect.
 */

/** How many sides each polygon has. */
export const POLYGON_SIDES = [5, 4, 4] as const;
/** Positions remembered per polygon, newest first. */
export const TRAIL_COPIES = 44;
/** Floats per edge handed to `fill`: both ends, how old (0 newest, 1 oldest) and which polygon. */
export const EDGE_STRIDE = 6;
export const MAX_EDGES = POLYGON_SIDES.reduce((total, sides) => total + sides * (TRAIL_COPIES + 1), 0);

/** How often a position is remembered, in seconds. */
const SNAPSHOT_EVERY_S = 0.02;
/** Corners stay this far inside the edges, in picture heights. */
const MARGIN = 0.94;
/** A kick turns every corner by up to this much and speeds it up, the speed-up dying away over this long. */
const KICK_TURN = 1.0;
const KICK_SPEED_UP = 2.4;
const KICK_DECAY_S = 0.3;

export class Polygons {
  private readonly x: Float32Array[];
  private readonly y: Float32Array[];
  private readonly heading: Float32Array[];
  /** Per polygon, `TRAIL_COPIES` snapshots of x then y for every corner, in a ring. */
  private readonly trail: Float32Array[];
  private newest = 0;
  private remembered = 0;
  private sinceSnapshot = 0;
  private impulse = 0;
  private readonly random: () => number;

  constructor(random: () => number = Math.random) {
    this.random = random;
    this.x = POLYGON_SIDES.map((sides) => new Float32Array(sides));
    this.y = POLYGON_SIDES.map((sides) => new Float32Array(sides));
    this.heading = POLYGON_SIDES.map((sides) => new Float32Array(sides));
    this.trail = POLYGON_SIDES.map((sides) => new Float32Array(TRAIL_COPIES * sides * 2));
    for (let polygon = 0; polygon < POLYGON_SIDES.length; polygon++) {
      for (let corner = 0; corner < POLYGON_SIDES[polygon]; corner++) {
        this.x[polygon][corner] = (random() * 2 - 1) * MARGIN;
        this.y[polygon][corner] = (random() * 2 - 1) * MARGIN;
        this.heading[polygon][corner] = random() * Math.PI * 2;
      }
    }
  }

  /** Moves every corner for `dt` seconds at `speed` picture heights a second. */
  step(dt: number, aspect: number, speed: number): void {
    const reach = MARGIN * aspect;
    const travelled = speed * (1 + KICK_SPEED_UP * this.impulse) * dt;
    this.impulse *= Math.exp(-dt / KICK_DECAY_S);
    for (let polygon = 0; polygon < POLYGON_SIDES.length; polygon++) {
      const x = this.x[polygon];
      const y = this.y[polygon];
      const heading = this.heading[polygon];
      for (let corner = 0; corner < x.length; corner++) {
        let dx = Math.cos(heading[corner]);
        let dy = Math.sin(heading[corner]);
        x[corner] += dx * travelled;
        y[corner] += dy * travelled;
        // A corner that has gone past an edge is put back on it, and sent the other way.
        if (x[corner] > reach || x[corner] < -reach) {
          x[corner] = Math.max(-reach, Math.min(reach, x[corner]));
          dx = -dx;
        }
        if (y[corner] > MARGIN || y[corner] < -MARGIN) {
          y[corner] = Math.max(-MARGIN, Math.min(MARGIN, y[corner]));
          dy = -dy;
        }
        heading[corner] = Math.atan2(dy, dx);
      }
    }

    this.sinceSnapshot += dt;
    // A stall does not leave a gap in the trail, but it does not fill the whole of it either.
    for (let made = 0; this.sinceSnapshot >= SNAPSHOT_EVERY_S && made < 3; made++) {
      this.sinceSnapshot -= SNAPSHOT_EVERY_S;
      this.remember();
    }
    this.sinceSnapshot = Math.min(this.sinceSnapshot, SNAPSHOT_EVERY_S);
  }

  /** A beat: every corner swerves and, for a moment, speeds up. `strength` is 0..1. */
  kick(strength: number): void {
    this.impulse = Math.max(this.impulse, strength);
    for (let polygon = 0; polygon < POLYGON_SIDES.length; polygon++) {
      const heading = this.heading[polygon];
      for (let corner = 0; corner < heading.length; corner++) {
        heading[corner] += (this.random() * 2 - 1) * KICK_TURN * strength;
      }
    }
  }

  /** Writes every edge of every polygon and its trail into `out`; returns how many edges. */
  fill(out: Float32Array): number {
    let edges = 0;
    for (let polygon = 0; polygon < POLYGON_SIDES.length; polygon++) {
      const sides = POLYGON_SIDES[polygon];
      for (let copy = 0; copy <= this.remembered; copy++) {
        const age = copy / TRAIL_COPIES;
        for (let corner = 0; corner < sides; corner++) {
          const next = (corner + 1) % sides;
          const at = edges++ * EDGE_STRIDE;
          if (copy === 0) {
            out[at] = this.x[polygon][corner];
            out[at + 1] = this.y[polygon][corner];
            out[at + 2] = this.x[polygon][next];
            out[at + 3] = this.y[polygon][next];
          } else {
            const base = this.slot(copy - 1) * sides * 2;
            const trail = this.trail[polygon];
            out[at] = trail[base + corner];
            out[at + 1] = trail[base + sides + corner];
            out[at + 2] = trail[base + next];
            out[at + 3] = trail[base + sides + next];
          }
          out[at + 4] = age;
          out[at + 5] = polygon;
        }
      }
    }
    return edges;
  }

  /** The place in the ring of the snapshot `age` before the newest. */
  private slot(age: number): number {
    return (this.newest - age + TRAIL_COPIES) % TRAIL_COPIES;
  }

  private remember(): void {
    this.newest = (this.newest + 1) % TRAIL_COPIES;
    this.remembered = Math.min(TRAIL_COPIES, this.remembered + 1);
    for (let polygon = 0; polygon < POLYGON_SIDES.length; polygon++) {
      const base = this.newest * POLYGON_SIDES[polygon] * 2;
      this.trail[polygon].set(this.x[polygon], base);
      this.trail[polygon].set(this.y[polygon], base + POLYGON_SIDES[polygon]);
    }
  }
}
