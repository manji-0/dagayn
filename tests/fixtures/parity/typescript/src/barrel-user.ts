import { fromA, renamedB, localRenamed, bns, ClassA, defB } from "./barrel";
export function useBarrel() {
  fromA();
  renamedB();
  localRenamed();
  bns.fromB();
  new ClassA();
  defB();
}
