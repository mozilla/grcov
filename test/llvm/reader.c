int foo(int x, int n)
{
    int i;
    for (i = 0; i < n; ++i) {
        if (x > n)
        {
            x += i;
        }
        else
        {
            int j;
            for (j = 0; j < i; ++j) {
                x += j;
            }
        }
    }
    return x;
}

int main()
{
    int x = foo(10, 10);
    int y = 0;
    int i;
    for (i = 0; i < x; ++i)
    {
        y += foo(x, 10);
    }
    return y;
}
