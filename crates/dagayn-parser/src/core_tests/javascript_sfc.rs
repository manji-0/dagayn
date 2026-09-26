use super::*;

#[test]
fn parses_vue_script_blocks_with_typescript_offsets() {
    let source = br#"<template>
  <div class="app">
    <UserList :users="users" @select="onSelectUser" />
  </div>
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'
import UserList from './UserList.vue'

interface User {
  id: number
  name: string
}

const count = ref(0)

function increment() {
  count.value++
}

function onSelectUser(user: User) {
  console.log(user.name)
}

const doubled = computed(() => count.value * 2)
</script>
"#;
    let (nodes, edges) = parse_vue("sample.vue", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "File" && node.name == "sample.vue" && node.language == "vue"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.language == "vue"
            && node.extra["type_role"] == "interface"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "increment"
            && node.language == "vue"
            && node.line_start == 18
    }));
    assert!(
        edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "vue" && edge.line == 8 })
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.vue" && edge.target == "vue::ref"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.vue::onSelectUser"
            && edge.target == "log"
            && edge.line == 23
    }));
}

#[test]
fn parses_svelte_script_blocks_with_typescript_offsets() {
    let source = br#"<script lang="ts">
import { writable } from 'svelte/store'

interface User {
  name: string
}

const count = writable(0)

function increment() {
  console.log('increment')
}

function selectUser(user: User) {
  return user.name
}
</script>

<button on:click={increment}>{$count}</button>
"#;
    let (nodes, edges) = parse_svelte("sample.svelte", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "File" && node.name == "sample.svelte" && node.language == "svelte"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.language == "svelte"
            && node.extra["type_role"] == "interface"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "increment"
            && node.language == "svelte"
            && node.line_start == 10
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.target == "svelte/store" && edge.line == 2
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.svelte"
            && edge.target == "svelte/store::writable"
            && edge.extra["external_package"] == "svelte"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.svelte::increment"
            && edge.target == "log"
            && edge.line == 11
    }));
}
