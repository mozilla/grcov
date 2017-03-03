#include "include.h"

void Ciao::setName(string n) {
  name = n;
}

string Ciao::getName() {
  calledFromFile();
  return name;
}

