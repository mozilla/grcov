#include <string>
#include <iostream>

using namespace std;

class Ciao {
  public:
    void setName(string n);
    string getName();
    void calledFromFile() {
      cout << name;
    }

  private:
    string name;
};
