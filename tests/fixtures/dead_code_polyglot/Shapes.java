package demo;

import org.springframework.web.bind.annotation.GetMapping;

public class Shapes extends Base {
    @Override
    public String toString() {
        return "s";
    }

    @GetMapping(
        value = "/shapes")
    public String listShapes() {
        return "";
    }

    public boolean equals(Object other) {
        return false;
    }

    private void readObject(java.io.ObjectInputStream in) {
    }

    public void javaUnusedHelper() {
    }
}

interface Drawable {
    void draw();
}
