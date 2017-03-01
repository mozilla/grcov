#include <iostream>
#include <string>

using namespace std;

class Ciao {
public:
  void setName(string n) {
    name = n;
  }

  string getName() {
    return name;
  }

  void uncalled() {
    cout << name;
  }

private:
  string name;
};

int main(void)
{
  Ciao ciao;

  ciao.setName("Marco");
  cout << ciao.getName() << endl;

  if (ciao.getName() == "marco") {
    ciao.uncalled();
  }

  return 0;
}
