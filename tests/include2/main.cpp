#include "include.h"

int main(void)
{
  Ciao ciao;

  ciao.setName("marco");
  string n = ciao.getName();

  if (n == "prova") {
      ciao.calledFromFile();
  }

  return 0;
}
