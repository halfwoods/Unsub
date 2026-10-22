/**
 * How the Pipes style grows, apart from how it is drawn: where each pipe goes, when it turns, when
 * the space is full and the picture fades to start again. Pipes run along the lines of a grid, one
 * cell at a time, and never cross one another.
 */

export const GRID_X = 12;
export const GRID_Y = 8;
export const GRID_Z = 12;
export const MAX_PIPES = 4;
/** Floats in one drawn piece: centre or start and radius, then end and hue. */
export const PIECE_STRIDE = 8;
export const MAX_SEGMENTS = 2048;
export const MAX_JOINTS = 1024;

/** The pipes' radius against a cell, and a joint's against a pipe's. */
export const PIPE_RADIUS = 0.2;
const JOINT_RATIO = 1.35;
/** A pipe stops after this many cells, so the space is shared out between several. */
const MAX_LENGTH = 48;
const TURN_CHANCE = 0.26;
/** The picture is done once this share of the cells is taken, or after this long. */
const FULL_SHARE = 0.4;
const MAX_AGE_S = 75;
const FADE_OUT_S = 1.6;
const FADE_IN_S = 0.9;
/** Where each pipe's hue sits against the music's, so pipes alive together differ. */
const HUE_OFFSETS = [0, 0.37, 0.17, 0.55];
const MIN_PIPES = 2;

const DIRECTIONS = [
  [1, 0, 0],
  [-1, 0, 0],
  [0, 1, 0],
  [0, -1, 0],
  [0, 0, 1],
  [0, 0, -1],
] as const;

interface Pipe {
  slot: number;
  x: number;
  y: number;
  z: number;
  direction: number;
  /** How far along the cell it is growing into, 0..1. */
  progress: number;
  length: number;
  forceTurn: boolean;
}

export class PipeField {
  /** Pieces that are finished: cylinders (start, radius, end, hue) and balls at corners and ends (centre, radius, hue). */
  readonly segments = new Float32Array(MAX_SEGMENTS * PIECE_STRIDE);
  segmentCount = 0;
  readonly joints = new Float32Array(MAX_JOINTS * PIECE_STRIDE);
  jointCount = 0;
  /** The cylinders still growing, and the balls on their tips. Rewritten by every `step`. */
  readonly heads = new Float32Array(MAX_PIPES * PIECE_STRIDE);
  readonly tips = new Float32Array(MAX_PIPES * PIECE_STRIDE);
  headCount = 0;
  /** 1 while the picture is whole, falling to 0 as it is cleared and back to 1 as a new one starts. */
  fade = 1;
  /** Whether the space is being cleared, or has just been. */
  clearing = false;

  private readonly occupied = new Uint8Array(GRID_X * GRID_Y * GRID_Z);
  private taken = 0;
  private pipes: Pipe[] = [];
  private age = 0;
  private hue = 0;
  private random: () => number;

  constructor(seed = 1) {
    this.random = mulberry32(seed);
    this.begin();
  }

  get pipeCount(): number {
    return this.pipes.length;
  }

  /** The share of the cells that pipes have taken. */
  get fullness(): number {
    return this.taken / this.occupied.length;
  }

  /** The cell at the node's coordinates, in world units: the grid is centred on the origin. */
  static world(index: number, size: number): number {
    return index - (size - 1) / 2;
  }

  /**
   * Grows every pipe by `dt` seconds at `speed` cells a second. `hue` (0..1) is what the pipes are
   * coloured with from here on, each shifted by its own offset.
   */
  step(dt: number, speed: number, hue: number): void {
    this.hue = hue;
    if (this.clearing) {
      this.fade -= dt / FADE_OUT_S;
      if (this.fade <= 0) {
        this.fade = 0;
        this.begin();
      }
    } else {
      this.fade = Math.min(1, this.fade + dt / FADE_IN_S);
      this.age += dt;
      // Backwards, because a pipe that ends is taken out of the list as it is reached.
      for (let i = this.pipes.length - 1; i >= 0; i--) this.grow(this.pipes[i], dt * speed);
      // Keeps the space busy when a pipe has run into a dead end.
      while (this.pipes.length < MIN_PIPES && this.spawn());
      if (this.isDone()) this.clearing = true;
    }
    this.describeHeads();
  }

  /** A beat: a new pipe starts, or if there are as many as there can be, one of them turns. */
  beat(): void {
    if (this.clearing) return;
    if (this.pipes.length < MAX_PIPES) {
      this.spawn();
    } else {
      this.pipes[Math.floor(this.random() * this.pipes.length)].forceTurn = true;
    }
  }

  private begin(): void {
    this.occupied.fill(0);
    this.taken = 0;
    this.segmentCount = 0;
    this.jointCount = 0;
    this.pipes = [];
    this.age = 0;
    this.clearing = false;
    for (let i = 0; i < MIN_PIPES; i++) this.spawn();
    this.describeHeads();
  }

  private isDone(): boolean {
    return (
      this.age > MAX_AGE_S ||
      this.fullness >= FULL_SHARE ||
      this.segmentCount > MAX_SEGMENTS - MAX_PIPES ||
      this.jointCount > MAX_JOINTS - 2 * MAX_PIPES ||
      this.pipes.length === 0
    );
  }

  private cell(x: number, y: number, z: number): number {
    return x + GRID_X * (y + GRID_Y * z);
  }

  private isFree(x: number, y: number, z: number): boolean {
    return (
      x >= 0 && x < GRID_X && y >= 0 && y < GRID_Y && z >= 0 && z < GRID_Z && this.occupied[this.cell(x, y, z)] === 0
    );
  }

  private take(x: number, y: number, z: number): void {
    this.occupied[this.cell(x, y, z)] = 1;
    this.taken += 1;
  }

  /** A pipe in a free cell, going the way it has room for. False if there is no room anywhere. */
  private spawn(): boolean {
    let slot = 0;
    while (slot < HUE_OFFSETS.length && this.pipes.some((pipe) => pipe.slot === slot)) slot++;
    if (slot === HUE_OFFSETS.length) return false;
    for (let attempt = 0; attempt < 40; attempt++) {
      const x = Math.floor(this.random() * GRID_X);
      const y = Math.floor(this.random() * GRID_Y);
      const z = Math.floor(this.random() * GRID_Z);
      if (!this.isFree(x, y, z)) continue;
      const pipe: Pipe = { slot, x, y, z, direction: 0, progress: 0, length: 0, forceTurn: false };
      const open = this.open(pipe);
      if (open === 0) continue;
      pipe.direction = this.pickFrom(open, 0);
      this.take(x, y, z);
      this.claimAhead(pipe);
      this.addJoint(pipe, x, y, z);
      this.pipes.push(pipe);
      return true;
    }
    return false;
  }

  /** The directions `pipe` has room to go in, as bits. */
  private open(pipe: Pipe): number {
    let open = 0;
    for (let direction = 0; direction < DIRECTIONS.length; direction++) {
      const [dx, dy, dz] = DIRECTIONS[direction];
      if (this.isFree(pipe.x + dx, pipe.y + dy, pipe.z + dz)) open |= 1 << direction;
    }
    return open;
  }

  /** One of the directions in `open` at random, leaving out those in `except`. */
  private pickFrom(open: number, except: number): number {
    const choices = open & ~except;
    let wanted = Math.floor(this.random() * bitCount(choices));
    for (let direction = 0; direction < DIRECTIONS.length; direction++) {
      if ((choices & (1 << direction)) !== 0 && wanted-- === 0) return direction;
    }
    return -1;
  }

  /**
   * The direction to go on in from where `pipe` is, or -1 if it is boxed in: straight on mostly,
   * round a corner now and then or when told to, and round a corner when it must.
   */
  private choose(pipe: Pipe, turn: boolean): number {
    const open = this.open(pipe);
    if (open === 0) return -1;
    const straight = (open & (1 << pipe.direction)) !== 0;
    const corners = open & ~(1 << pipe.direction);
    if (corners !== 0 && (turn || !straight || this.random() < TURN_CHANCE)) return this.pickFrom(corners, 0);
    return pipe.direction;
  }

  /** Takes the cell the pipe is growing into, so no other pipe can head for it. */
  private claimAhead(pipe: Pipe): void {
    const [dx, dy, dz] = DIRECTIONS[pipe.direction];
    this.take(pipe.x + dx, pipe.y + dy, pipe.z + dz);
  }

  private grow(pipe: Pipe, cells: number): void {
    pipe.progress += cells;
    while (pipe.progress >= 1) {
      pipe.progress -= 1;
      const [dx, dy, dz] = DIRECTIONS[pipe.direction];
      this.addSegment(pipe, pipe.x, pipe.y, pipe.z, pipe.x + dx, pipe.y + dy, pipe.z + dz);
      pipe.x += dx;
      pipe.y += dy;
      pipe.z += dz;
      pipe.length += 1;

      const direction = pipe.length >= MAX_LENGTH ? -1 : this.choose(pipe, pipe.forceTurn);
      if (direction < 0) {
        this.addJoint(pipe, pipe.x, pipe.y, pipe.z);
        this.pipes.splice(this.pipes.indexOf(pipe), 1);
        return;
      }
      if (direction !== pipe.direction) this.addJoint(pipe, pipe.x, pipe.y, pipe.z);
      pipe.forceTurn = false;
      pipe.direction = direction;
      this.claimAhead(pipe);
    }
  }

  private pipeHue(pipe: Pipe): number {
    return this.hue + HUE_OFFSETS[pipe.slot];
  }

  private addSegment(pipe: Pipe, ax: number, ay: number, az: number, bx: number, by: number, bz: number): void {
    if (this.segmentCount >= MAX_SEGMENTS) return;
    put(
      this.segments,
      this.segmentCount++,
      PipeField.world(ax, GRID_X),
      PipeField.world(ay, GRID_Y),
      PipeField.world(az, GRID_Z),
      PIPE_RADIUS,
      PipeField.world(bx, GRID_X),
      PipeField.world(by, GRID_Y),
      PipeField.world(bz, GRID_Z),
      this.pipeHue(pipe),
    );
  }

  private addJoint(pipe: Pipe, x: number, y: number, z: number): void {
    if (this.jointCount >= MAX_JOINTS) return;
    const [wx, wy, wz] = [PipeField.world(x, GRID_X), PipeField.world(y, GRID_Y), PipeField.world(z, GRID_Z)];
    put(this.joints, this.jointCount++, wx, wy, wz, PIPE_RADIUS * JOINT_RATIO, wx, wy, wz, this.pipeHue(pipe));
  }

  /** Fills `heads` and `tips` from the pipes as they are now. */
  private describeHeads(): void {
    this.headCount = this.pipes.length;
    for (let index = 0; index < this.pipes.length; index++) {
      const pipe = this.pipes[index];
      const [dx, dy, dz] = DIRECTIONS[pipe.direction];
      const sx = PipeField.world(pipe.x, GRID_X);
      const sy = PipeField.world(pipe.y, GRID_Y);
      const sz = PipeField.world(pipe.z, GRID_Z);
      const tx = sx + dx * pipe.progress;
      const ty = sy + dy * pipe.progress;
      const tz = sz + dz * pipe.progress;
      const hue = this.pipeHue(pipe);
      put(this.heads, index, sx, sy, sz, PIPE_RADIUS, tx, ty, tz, hue);
      // A ball the pipe's own width closes the open end without showing as a bulge.
      put(this.tips, index, tx, ty, tz, PIPE_RADIUS, tx, ty, tz, hue);
    }
  }
}

function put(
  out: Float32Array,
  index: number,
  ax: number,
  ay: number,
  az: number,
  aw: number,
  bx: number,
  by: number,
  bz: number,
  bw: number,
): void {
  const at = index * PIECE_STRIDE;
  out[at] = ax;
  out[at + 1] = ay;
  out[at + 2] = az;
  out[at + 3] = aw;
  out[at + 4] = bx;
  out[at + 5] = by;
  out[at + 6] = bz;
  out[at + 7] = bw;
}

function bitCount(bits: number): number {
  let count = 0;
  for (; bits !== 0; bits &= bits - 1) count++;
  return count;
}

/** A small seeded generator, so a picture can be made again exactly. */
function mulberry32(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
