int foo(int x) {
    if (x > 0 &&
        x <= 3) {
        int y = x + 1;
        y += 1;
        return y;
    } else {
        return 1;
    }
}

int bar(int x) {
    if (x > 0) {
        return 0;
    } else {
        return 1;
    }
}

int oof(int x) {
    if (x > 0 && x <= 3) {
        return 0;
    } else {
        int y = x + 1;
        y += 1;
        return y;
    }
}


int main() {
    return foo(-1) + foo(2) + bar(-1);
}
