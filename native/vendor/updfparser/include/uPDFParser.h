/*
  Copyright 2021 Grégory Soutadé

  This file is part of uPDFParser.

  uPDFParser is free software: you can redistribute it and/or modify
  it under the terms of the GNU Lesser General Public License as published by
  the Free Software Foundation, either version 3 of the License, or
  (at your option) any later version.

  uPDFParser is distributed in the hope that it will be useful,
  but WITHOUT ANY WARRANTY; without even the implied warranty of
  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
  GNU Lesser General Public License for more details.

  You should have received a copy of the GNU Lesser General Public License
  along with uPDFParser. If not, see <http://www.gnu.org/licenses/>.
*/

#ifndef _UPDFPARSER_HPP_
#define _UPDFPARSER_HPP_

#include <exception>
#include <map>
#include <vector>
#include <string>
#include <sstream>
#include <iostream>
#include <iomanip>
#include <string.h>
#include <unistd.h>

#include "uPDFTypes.h"
#include "uPDFObject.h"

namespace uPDFParser
{
    class XRefValue;
    
    /**
     * @brief PDF Parser
     */
    class Parser
    {
    public:
	Parser(int version_major=1, int version_minor=6):
	    version_major(version_major), version_minor(version_minor),
	    xrefObject(0), xrefOffset((off_t)-1), fd(0), curOffset(0)
	{}

	~Parser()
	{
	    if (fd) close(fd);
	    
	    std::vector<Object*>::iterator it;
	    for(it=_objects.begin(); it!=_objects.end(); it++)
		delete *it;
	}

	/**
	 * @brief Parse a file
	 */
	void parse(const std::string& filename);

	/**
	 * @brief Write a PDF file with internal objects
	 *
	 * @param filename File path
	 * @param update   Only append new objects if true
	 *                 Write a new PDF file if false (not supported for now)
	 */
	void write(const std::string& filename, bool update=false);

	/**
	 * @brief Get internals (or parsed) objects
	 */
	std::vector<Object*>& objects() { return _objects; }

	/**
	 * @brief Add an object
	 */
	void addObject(Object* object) { _objects.push_back(object); }

	/**
	 * @brief Remove an object from list and crefTable
	 */
	void removeObject(Object* object);

	/**
	 * @brief Return trailer object
	 */
	Object& getTrailer() {return trailer; }

	/**
	 * @brief Return xref table. This table is read and updated only once after parse
	 * It's not used for write operation
	 */
	const std::vector<XRefValue>& xrefTable() {return _xrefTable;}

	/**
	 * @brief Return a specific object
	 */
	Object* getObject(int objectId, int generationNumber=0);
	
    private:
	void parseObject(std::string& token);
	void parseHeader();
	void parseStartXref();
	bool parseXref();
	bool parseTrailer();

	char prevChar();
	std::string nextToken(bool exceptionOnEOF=true, bool readComment=false);
	
	DataType* parseType(std::string& token, Object* object, std::map<std::string, DataType*>& dict);
	void parseDictionary(Object* object, std::map<std::string, DataType*>& dict);
	DataType* parseSignedNumber(std::string& token);
	DataType* parseNumber(std::string& token);
	DataType* parseNumberOrReference(std::string& token);
	Array* parseArray(Object* object);
	String* parseString();
	HexaString* parseHexaString();
	Stream* parseStream(Object* object);
	Name* parseName(std::string& token);

	void repairTrailer();
	void writeBuffer(int fd, const char* buffer, int size);
	void writeUpdate(const std::string& filename);

	char c;
	int version_major, version_minor;
	std::vector<Object*> _objects;
	Object trailer, *xrefObject;
	off_t xrefOffset;
	int fd;
	off_t curOffset;
	std::vector<XRefValue> _xrefTable;
    };

    class XRefValue
    {
    public:
	XRefValue(int objectId, int offset, int generationNumber, bool used, Object* object=0):
	    _objectId(objectId), _offset(offset), _generationNumber(generationNumber), _used(used),
	    _object(object)
	{}

	int objectId() {return _objectId;}
	int offset() {return _offset;}
	int generationNumber() {return _generationNumber;}
	bool used() {return _used;}
	
	void setObject(Object* object) { _object = object; }
	Object* object() { return _object; }
	
    private:
	int _objectId;
	int _offset;
	int _generationNumber;
	bool _used;
	Object* _object;
    };
}

#endif
