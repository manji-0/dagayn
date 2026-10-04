package main

type Point struct{ X int }

func (p Point) String() string { return "p" }

func init() {}

func (p Point) goUnusedHelper() int { return p.X }

func goUnusedFunc() {}
