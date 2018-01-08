#include "llvm/Support/Errc.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/Path.h"
#include "GCOV.h"

using namespace llvm;

class CustomFileInfo : public ProtectedFileInfo {
public:
  CustomFileInfo(const GCOV::Options &Options) : ProtectedFileInfo(Options) {}

  void printIntermediate(StringRef MainFilename);
};

/// printIntermediate -  Print source files with collected line count information in the intermediate gcov format.
void CustomFileInfo::printIntermediate(StringRef MainFilename) {
  std::string CoveragePath = getCoveragePath(MainFilename, MainFilename);
  std::unique_ptr<raw_ostream> CovStream = openCoveragePath(CoveragePath);
  raw_ostream &CovOS = *CovStream;

  SmallVector<StringRef, 4> Filenames;
  for (const auto &LI : LineInfo)
    Filenames.push_back(LI.first());
  std::sort(Filenames.begin(), Filenames.end());

  for (StringRef Filename : Filenames) {
    CovOS << "file:" << Filename << "\n";

    const LineData &Line = LineInfo[Filename];
    for (uint32_t LineIndex = 0; LineIndex < Line.LastLine; ++LineIndex) {
      FunctionLines::const_iterator FuncsIt = Line.Functions.find(LineIndex);
      if (FuncsIt != Line.Functions.end()) {
        for (const CustomGCOVFunction *Func : FuncsIt->second) {
          CovOS << "function:" << (LineIndex + 1) << "," << Func->getEntryCount() << "," << Func->getName() << "\n";
        }
      }

      BlockLines::const_iterator BlocksIt = Line.Blocks.find(LineIndex);
      if (BlocksIt == Line.Blocks.end()) {
        // No basic blocks are on this line. Not an executable line of code.
        continue;
      } else {
        const BlockVector &Blocks = BlocksIt->second;

        // Add up the block counts to form line counts.
        DenseMap<const CustomGCOVFunction *, bool> LineExecs;
        uint64_t LineCount = 0;
        for (const CustomGCOVBlock *Block : Blocks) {
          LineCount += Block->getCount();
        }

        CovOS << "lcount:" << (LineIndex + 1) << "," << LineCount << "\n";

        if (Options.BranchInfo) {
          for (const CustomGCOVBlock *Block : Blocks) {
            // Only print block and branch information at the end of the block.
            if (Block->getLastLine() != LineIndex + 1)
              continue;

            size_t NumEdges = Block->getNumDstEdges();
            if (NumEdges > 1) {
              uint64_t TotalCounts = 0;
              for (const GCOVEdge *Edge : Block->dsts()) {
                TotalCounts += Edge->Count;
              }
              bool exec = TotalCounts > 0;
              for (const GCOVEdge *Edge : Block->dsts()) {
                bool taken = Edge->Count > 0;
                CovOS << "branch:" << (LineIndex + 1) << ",";
                if (taken && exec)
                  CovOS << "taken";
                else if (exec)
                  CovOS << "nottaken";
                else
                  CovOS << "notexec";
                CovOS << "\n";
              }
            }
          }
        }
      }
    }
  }
}

extern "C"
void parse_llvm_gcno(char* file_stem, uint8_t branch_enabled) {
  GCOV::Options Options(
    /* AllBlocks */ false,
    /* BranchProb (BranchInfo) */ branch_enabled != 0,
    /* BranchCount */ branch_enabled != 0,
    /* FuncSummary (FuncCoverage) */ false,
    /* PreservePaths */ false,
    /* UncondBranch */ false,
    /* LongNames */ false,
    /* NoOutput */ false
  );

  CustomGCOVFile GF;

  std::string GCNO = std::string(file_stem) + ".gcno";
  std::string GCDA = std::string(file_stem) + ".gcda";

  ErrorOr<std::unique_ptr<MemoryBuffer>> GCNO_Buff = MemoryBuffer::getFileOrSTDIN(GCNO);
  if (std::error_code EC = GCNO_Buff.getError()) {
    errs() << GCNO << ": " << EC.message() << "\n";
    return;
  }
  GCOVBuffer GCNO_GB(GCNO_Buff.get().get());
  if (!GF.readGCNO(GCNO_GB)) {
    errs() << "Invalid .gcno File!\n";
    return;
  }

  ErrorOr<std::unique_ptr<MemoryBuffer>> GCDA_Buff = MemoryBuffer::getFileOrSTDIN(GCDA);
  if (std::error_code EC = GCDA_Buff.getError()) {
    if (EC != errc::no_such_file_or_directory) {
      errs() << GCDA << ": " << EC.message() << "\n";
      return;
    }
    // Clear the filename to make it clear we didn't read anything.
    GCDA = "-";
  } else {
    GCOVBuffer GCDA_GB(GCDA_Buff.get().get());
    if (!GF.readGCDA(GCDA_GB)) {
      errs() << "Invalid .gcda File!\n";
      return;
    }
  }

  CustomFileInfo FI(Options);
  GF.collectLineCounts(FI);
  FI.printIntermediate(GCNO);
}
