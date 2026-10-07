int called(int x)
{
    int i, r = 0;
    for (i = 0; i < x; ++i) {
        r += i;
    }
    return r;
}

int never_called(int x)
{
    if (x > 0) {
        return x + 1;
    }
    return -x;
}

int main()
{
    return called(3) - 3;
}
