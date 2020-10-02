#include <string>

using namespace std;

template <class T>
class Ciao {
  private:
    T val;

  public:
    void set(T v) {
      val = v;
    }

    T get() {
      return val;
    }

    T* get2() {
      return &val;
    }
};

int main() {
  Ciao<wstring> cW;
  Ciao<string> cS;

  cS.set("marco");
  cW.get();
  cS.get2();
  cW.get2();
}
