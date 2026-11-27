#include <cstdio>

static int check(int a, int b, int c) {
    if (a && (b || c)) {
        return 1;
    }
    return 0;
}

static int unused(int a, int b) {
    if (a || b) {
        return 2;
    }
    return 3;
}

int main(int argc, char **argv) {
    int n = check(argc, 0, 1) + check(0, argc, 0);
    if (argc > 5 && n > 1) {
        n += unused(argc, n);
    }
    printf("%d\n", n);
    return 0;
}
