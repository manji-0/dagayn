#define PY_SSIZE_T_CLEAN
#include <Python.h>

typedef struct TSLanguage TSLanguage;

const TSLanguage *tree_sitter_markdown(void);

static PyObject *language(PyObject *self, PyObject *args) {
    (void)self;
    (void)args;
    return PyCapsule_New(
        (void *)tree_sitter_markdown(),
        "tree_sitter.Language",
        NULL
    );
}

static PyMethodDef methods[] = {
    {"language", language, METH_NOARGS, "Return the tree-sitter Language capsule."},
    {NULL, NULL, 0, NULL},
};

static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT,
    "markdown",
    NULL,
    -1,
    methods,
};

PyMODINIT_FUNC PyInit_markdown(void) {
    return PyModule_Create(&module);
}
