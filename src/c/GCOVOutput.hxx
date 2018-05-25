#ifndef GCOVOUTPUT_HXX
#define GCOVOUTPUT_HXX

extern "C" void handleFileRust(void*, const char*, size_t);
extern "C" void handleFunctionRust(void*, uint32_t, uint64_t, const char*, size_t);
extern "C" void handleLcountRust(void*, uint32_t, uint64_t);
extern "C" void handleBranchRust(void*, uint32_t, uint8_t, uint8_t);

namespace llvm {

    class GCOVOutputStream
    {
        raw_ostream & CovOS;

    public:
        GCOVOutputStream(raw_ostream & _CovOS) : CovOS(_CovOS) { }

        void handleFile(StringRef filename)
            {
                CovOS << "file:" << filename << "\n";
            }

        void handleFunction(uint32_t index, uint64_t entrycount, StringRef funcname)
            {
                CovOS << "function:" << index << "," << entrycount << "," << funcname << "\n";
            }

        void handleLcount(uint32_t index, uint64_t linecount)
            {
                CovOS << "lcount:" << index << "," << linecount << "\n";
            }

        void handleBranch(uint32_t index, bool taken, bool exec)
            {
                CovOS << "branch:" << index << ",";
                if (taken && exec)
                    CovOS << "taken";
                else if (exec)
                    CovOS << "nottaken";
                else
                    CovOS << "notexec";
                CovOS << "\n";
            }
    };

    class GCOVOutputRust
    {

        void * RustHDL;

    public:
        GCOVOutputRust(void * _RustHDL) : RustHDL(_RustHDL) { }

        void handleFile(StringRef filename)
            {
                handleFileRust(RustHDL, filename.data(), filename.size());
            }

        void handleFunction(uint32_t index, uint64_t entrycount, StringRef funcname)
            {
                handleFunctionRust(RustHDL, index, entrycount, funcname.data(), funcname.size());
            }

        void handleLcount(uint32_t index, uint64_t linecount)
            {
                handleLcountRust(RustHDL, index, linecount);
            }

        void handleBranch(uint32_t index, bool taken, bool exec)
            {
                handleBranchRust(RustHDL, index, (uint8_t)taken, (uint8_t)exec);
            }
    };

}

#endif // GCOVOUTPUT_HXX
