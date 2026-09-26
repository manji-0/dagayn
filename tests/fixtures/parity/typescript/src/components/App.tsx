import React, { useState, useEffect } from "react";
import Card, { Button, IconButton } from "./Button";
import * as UI from "./Button";

function useCounter(initial: number) {
  const [n, setN] = useState(initial);
  useEffect(() => { setN(1); }, []);
  return n;
}

export function App() {
  const n = useCounter(0);
  const handle = () => console.log(n);
  return (
    <div>
      <Button label="a" onClick={handle} />
      <IconButton label="b" />
      <UI.Button label="c" />
      <Card></Card>
      <Local />
    </div>
  );
}
const Local = () => <span />;
export class ClassComp extends React.Component<{}> {
  render() { return <Button label="x" />; }
}
export const Memo = React.memo(function MemoInner() { return <div />; });
export const Fwd = React.forwardRef<HTMLDivElement>((props, ref) => <div ref={ref} />);
