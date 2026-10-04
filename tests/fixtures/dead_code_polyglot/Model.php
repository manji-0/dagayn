<?php
class Model {
    public function __construct() {}

    public function __toString(): string { return "m"; }

    #[Route('/x')]
    public function routed() {}

    public function phpUnusedHelper() {}
}
