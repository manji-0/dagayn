import def from "./default-arrow";
import Anon from "./default-anon-class";
import { decl as renamed, arrow } from "./functions";
import * as types from "./types";
import "./side-effect";
import type { Repo } from "./interfaces";
import { type Logger, Service } from "./interfaces";
import fs = require("fs");
import lib = require("./lib/base");
import Alias = Outer.Deep;
import { Outer } from "./namespaces";
import { util } from "@lib/base";
import React, { useState } from "react";
import { thing } from "./esm-compat.js";
import { Mixin } from "./lib";

export function useImports(r: Repo, l: Logger) {
  def(1);
  new Anon().hello();
  renamed(1);
  arrow(2);
  types.colorName(types.Color.Red);
  fs.readFileSync("x");
  lib.util(1);
  Alias.deepFn();
  util(3);
  useState(0);
  thing();
  Mixin;
}
