import type { Repo, Logger } from "./interfaces";
import { Color, UserId, Shape } from "./types";
import { Box } from "./classes";

export function typed(r: Repo, id: UserId): Shape {
  const l = {} as Logger;
  const c = { kind: "circle", r: 1 } satisfies Shape;
  let t: typeof Color = Color;
  const boxes: Array<Box<object>> = [];
  const x = <UserId>"a";
  return c;
}
export function constrained<T extends Repo = Repo>(x: T): x is T { return true; }
export class Holder { repo!: Repo; list: Map<UserId, Box<object>> = new Map(); }
