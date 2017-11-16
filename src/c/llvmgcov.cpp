#include "llvm/Support/Errc.h"
#include "GCOV.h"

using namespace llvm;

extern "C"
void parse_llvm_gcno(char* working_dir, char* file_stem) {
  GCOV::Options Options(
    /* AllBlocks */ false,
    /* BranchProb */ true,
    /* BranchCount */ true,
    /* FuncSummary */ false,
    /* PreservePaths */ false,
    /* UncondBranch */ false,
    /* LongNames */ false,
    /* NoOutput */ false
  );

  CustomGCOVFile GF;

  std::string SourceFile = std::string(file_stem) + ".gcno";
  std::string GCNO = SourceFile;
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
  FI.print(working_dir, llvm::outs(), SourceFile, GCNO, GCDA);
}
