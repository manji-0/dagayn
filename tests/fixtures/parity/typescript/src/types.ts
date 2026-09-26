export type UserId = string;
export type Shape = { kind: "circle"; r: number } | { kind: "sq"; s: number };
export type ReadonlyAll<T> = { readonly [K in keyof T]: T[K] };
export type Unwrap<T> = T extends Promise<infer U> ? U : T;
export type Handler = (req: Request) => Promise<Response>;

export enum Color { Red, Green = "g", Blue = 1 << 2 }
export const enum Direction { Up = 1, Down }
declare enum Ambient { A }

export function colorName(c: Color): string {
  return Color[c];
}
