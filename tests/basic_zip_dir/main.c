#include <stdio.h>

int main(void)
{
  for (int i = 1; i < 10; i++) {
      if (i % 3 == 0) {
        printf ("%d is divisible by 3\n", i);
      }

      if (i % 11 == 0) {
        printf ("%d is divisible by 11\n", i);
      }
  }

  return 0;
}
