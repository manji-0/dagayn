import { Controller, Get, Post, Body, Injectable } from "@nestjs/common";
@Injectable()
export class UsersService { findAll() { return []; } }
@Controller("users")
export class UsersController {
  constructor(private readonly users: UsersService) {}
  @Get()
  findAll() { return this.users.findAll(); }
  @Post()
  create(@Body() dto: CreateUserDto) { return dto; }
}
export class CreateUserDto { name!: string; }
