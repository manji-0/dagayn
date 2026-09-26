import { Component, OnInit } from "@angular/core";
@Component({ selector: "app-hero", template: "<p></p>" })
export class HeroComponent implements OnInit {
  ngOnInit(): void { this.load(); }
  load() {}
}
